use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use tokio::sync::{Notify, RwLock};
use tokio::time::{Duration, sleep};
use tracing::{error, info};

use crate::agent_browser::client::AgentBrowserClient;
use crate::agent_browser::types::AgentBrowserOptions;
use crate::config::AppConfig;
use crate::errors::AppResult;
use crate::scraper;
use crate::server_store::{ServerStore, ChannelInfo};

async fn monitor_channel(
    config: &AppConfig,
    server_store: &ServerStore,
    channel: &ChannelInfo,
) -> AppResult<()> {
    let client = AgentBrowserClient::new(AgentBrowserOptions {
        binary: config.agent_browser.binary.clone(),
        session_name: config.agent_browser.session_name.clone(),
        timeout_secs: config.agent_browser.timeout_secs,
    });

    let guild_id = config.discord.guild_id();
    scraper::open_channel(&client, &channel.channel_url, &guild_id, &channel.channel_id).await?;

    let store = server_store.get_message_store(&channel.channel_id)?;
    let existing_count = store.count().unwrap_or(0);

    let history = if existing_count == 0 {
        info!("Channel {} - full history scrape", channel.name);
        scraper::scrape_history(&client, config.scraper.max_history_pages).await?
    } else {
        info!("Channel {} - catch up ({} existing)", channel.name, existing_count);
        scraper::catch_up_to_bottom(&client).await?
    };

    let inserted = store.insert_batch(&history)?;
    info!("Channel {} - collected {}, inserted {}", channel.name, history.len(), inserted);

    // Poll once
    let latest_id = store.get_latest_discord_id().unwrap_or(None);
    let messages = scraper::poll_new_messages(&client, latest_id.as_deref()).await?;
    if !messages.is_empty() {
        let inserted = store.insert_batch(&messages)?;
        if inserted > 0 {
            info!("Channel {} - poll: {} new", channel.name, inserted);
        }
    }

    Ok(())
}
/// Process pending resyncs. Returns the set of channel IDs that were successfully resynced.
async fn process_pending_resyncs(
    pending_resyncs: &Mutex<HashSet<String>>,
    config: &AppConfig,
    server_store: &ServerStore,
) -> HashSet<String> {
    let resyncs: Vec<String> = match pending_resyncs.lock() {
        Ok(p) => p.iter().cloned().collect(),
        Err(_) => return HashSet::new(),
    };

    if resyncs.is_empty() {
        return HashSet::new();
    }

    let all_channels = match server_store.get_all_channels() {
        Ok(chs) => chs,
        Err(e) => {
            error!("Failed to get channels for resync: {e}");
            return HashSet::new();
        }
    };

    let mut completed = HashSet::new();

    for ch_id in &resyncs {
        if let Some(ch) = all_channels.iter().find(|c| c.channel_id == *ch_id) {
            info!("Resync: re-scraping channel {} ({})", ch.name, ch.channel_id);
            match monitor_channel(config, server_store, ch).await {
                Ok(_) => {
                    completed.insert(ch_id.clone());
                }
                Err(e) => {
                    error!("Resync {} error (will retry next cycle): {e}", ch.channel_id);
                }
            }
        } else {
            // Channel not found, remove from queue
            completed.insert(ch_id.clone());
        }
    }

    // Only remove successfully completed resyncs from the queue
    if let Ok(mut pending) = pending_resyncs.lock() {
        for id in &completed {
            pending.remove(id);
        }
    }

    completed
}

/// Multi-channel monitor loop
pub async fn run_monitor_loop(
    config: Arc<RwLock<AppConfig>>,
    server_store: Arc<ServerStore>,
    notify: Arc<Notify>,
    pending_resyncs: Arc<Mutex<HashSet<String>>>,
) {
    info!("MonitorManager started");

    loop {
        // Handle any pending resyncs first
        let cfg = config.read().await.clone();
        let resynced = process_pending_resyncs(&pending_resyncs, &cfg, &server_store).await;

        let channels = match server_store.get_monitored_channels() {
            Ok(chs) => chs,
            Err(e) => {
                error!("Failed to get monitored channels: {e}");
                sleep(Duration::from_secs(10)).await;
                continue;
            }
        };

        if channels.is_empty() {
            info!("No channels monitored, waiting for notification...");
            notify.notified().await;
            continue;
        }

        info!("Monitoring {} channels", channels.len());

        for ch in &channels {
            // Check for pending resyncs between each channel — prioritize them
            if pending_resyncs.lock().map_or(false, |p| !p.is_empty()) {
                info!("Pending resync detected, interrupting normal cycle");
                break;
            }

            // Skip channels already handled by resync this cycle
            if resynced.contains(&ch.channel_id) {
                continue;
            }

            info!("Processing channel: {} ({})", ch.name, ch.channel_id);

            if let Err(e) = monitor_channel(&cfg, &server_store, ch).await {
                error!("Channel {} error: {e}", ch.channel_id);
            }
        }

        // Poll interval — re-read in case it was updated
        let poll_secs = config.read().await.scraper.poll_interval_secs;
        tokio::select! {
            _ = sleep(Duration::from_secs(poll_secs)) => {},
            _ = notify.notified() => {
                info!("Channel list changed, reloading...");
            }
        }
    }
}
