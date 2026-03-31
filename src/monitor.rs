use std::sync::Arc;

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
/// Multi-channel monitor loop
pub async fn run_monitor_loop(
    config: Arc<RwLock<AppConfig>>,
    server_store: Arc<ServerStore>,
    notify: Arc<Notify>,
) {
    info!("MonitorManager started");

    loop {
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

        // Snapshot config for this cycle
        let cfg = config.read().await.clone();

        for ch in &channels {
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
