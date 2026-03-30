use std::sync::{Arc, RwLock};

use tokio::time::{Duration, sleep};
use tracing::{error, info, warn};

use crate::agent_browser::client::AgentBrowserClient;
use crate::agent_browser::types::AgentBrowserOptions;
use crate::config::AppConfig;
use crate::db::MessageStore;
use crate::errors::AppResult;
use crate::scraper;

/// How many viewports to scroll back when polling for recent messages.
const POLL_PAGES: u64 = 3;

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

        // ── Phase 1: Full history scrape ─────────────────────────────────────
        self.set_status("loading_history");
        info!(
            "Starting initial history scrape (max_top_attempts={})...",
            self.config.scraper.initial_scroll_pages
        );
        let history =
            scraper::scrape_history(&client, self.config.scraper.initial_scroll_pages).await?;
        let inserted = self.store.insert_batch(&history)?;
        info!(
            "History scrape complete: {} messages scraped, {} new inserted",
            history.len(),
            inserted
        );

        // ── Phase 2: Polling loop ─────────────────────────────────────────────
        self.set_status("monitoring");
        info!(
            "Entering monitoring mode (interval={}s, poll_pages={})",
            self.config.scraper.poll_interval_secs, POLL_PAGES
        );
        let interval = Duration::from_secs(self.config.scraper.poll_interval_secs);

        loop {
            sleep(interval).await;

            // Quick check: compare the last visible Discord message ID with DB latest.
            // Only do the expensive multi-page scroll-back when something is actually new.
            let latest_id = self.store.get_latest_discord_id().unwrap_or(None);
            let has_new = scraper::check_has_new_messages(&client, latest_id.as_deref())
                .await
                .unwrap_or(true); // on error, fall through to scrape

            if !has_new {
                continue;
            }

            match scraper::scrape_recent(&client, POLL_PAGES).await {
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
