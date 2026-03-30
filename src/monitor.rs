use std::sync::{Arc, RwLock};

use tokio::sync::Notify;
use tokio::time::{Duration, sleep};
use tracing::{error, info, warn};

use crate::agent_browser::client::AgentBrowserClient;
use crate::agent_browser::types::AgentBrowserOptions;
use crate::config::AppConfig;
use crate::db::MessageStore;
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

pub struct Monitor {
    config: AppConfig,
    store: Arc<MessageStore>,
    status: Arc<RwLock<String>>,
}

impl Monitor {
    pub fn new(config: AppConfig, store: Arc<MessageStore>, status: Arc<RwLock<String>>) -> Self {
        Self { config, store, status }
    }

    fn set_status(&self, s: &str) {
        if let Ok(mut w) = self.status.write() {
            *w = s.to_string();
        }
    }

    pub async fn run(&self) -> AppResult<()> {
        let client = AgentBrowserClient::new(AgentBrowserOptions {
            binary: self.config.agent_browser.binary.clone(),
            session_name: self.config.agent_browser.session_name.clone(),
            timeout_secs: self.config.agent_browser.timeout_secs,
        });

        self.set_status("opening_channel");
        let (guild_id, channel_id) = self.config.discord.parse_ids();
        scraper::open_channel(
            &client,
            &self.config.discord.channel_url,
            &guild_id,
            &channel_id,
        )
        .await?;

        // ── Initial load: full scrape or fast catch-up ───────────────────────
        let existing_count = self.store.count().unwrap_or(0);
        let history = if existing_count == 0 {
            self.set_status("loading_history");
            info!("DB is empty — starting full history scrape...");
            scraper::scrape_history(&client, self.config.scraper.max_history_pages).await?
        } else {
            self.set_status("catching_up");
            info!(
                "DB has {} messages — catching up from current position...",
                existing_count
            );
            scraper::catch_up_to_bottom(&client).await?
        };
        let inserted = self.store.insert_batch(&history)?;
        info!(
            "Initial load complete: {} messages collected, {} new inserted",
            history.len(),
            inserted
        );

        // ── Phase 2: Polling loop ─────────────────────────────────────────────
        self.set_status("monitoring");
        info!(
            "Entering monitoring mode (interval={}s)",
            self.config.scraper.poll_interval_secs
        );
        let interval = Duration::from_secs(self.config.scraper.poll_interval_secs);

        loop {
            sleep(interval).await;

            let latest_id = self.store.get_latest_discord_id().unwrap_or(None);
            match scraper::poll_new_messages(&client, latest_id.as_deref()).await {
                Ok(messages) => {
                    if !messages.is_empty() {
                        match self.store.insert_batch(&messages) {
                            Ok(inserted) => {
                                if inserted > 0 {
                                    info!("Poll: {} new messages inserted", inserted);
                                }
                            }
                            Err(e) => {
                                warn!("DB insert failed: {e}");
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Scrape error: {e}");
                    sleep(Duration::from_secs(5)).await;
                }
            }
        }
    }
}

/// Multi-channel monitor loop
pub async fn run_monitor_loop(
    config: AppConfig,
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

        for ch in &channels {
            info!("Processing channel: {} ({})", ch.name, ch.channel_id);

            if let Err(e) = monitor_channel(&config, &server_store, ch).await {
                error!("Channel {} error: {e}", ch.channel_id);
            }
        }

        // Poll interval
        tokio::select! {
            _ = sleep(Duration::from_secs(config.scraper.poll_interval_secs)) => {},
            _ = notify.notified() => {
                info!("Channel list changed, reloading...");
            }
        }
    }
}
