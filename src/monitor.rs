use std::sync::{Arc, RwLock};

use tokio::time::{Duration, sleep};
use tracing::{error, info, warn};

use crate::agent_browser::client::AgentBrowserClient;
use crate::agent_browser::types::AgentBrowserOptions;
use crate::config::AppConfig;
use crate::db::MessageStore;
use crate::errors::AppResult;
use crate::scraper;


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
            scraper::scrape_history(&client).await?
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
