mod agent_browser;
mod auth;
mod config;
mod db;
mod embedded;
mod errors;
mod monitor;
mod scraper;
mod server;

use std::sync::{Arc, RwLock};

use clap::Parser;
use tokio::sync::Notify;
use tokio::time::{Duration, sleep};
use tracing::{error, info};

#[derive(Parser)]
#[command(name = "discord-channel-to-html")]
#[command(about = "Monitor a Discord channel and serve messages as HTML + JSON API")]
struct Cli {
    #[arg(long, help = "Override server host")]
    host: Option<String>,
    #[arg(long, help = "Override server port")]
    port: Option<u16>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    let mut config = match config::load_or_init().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load config: {e}");
            std::process::exit(1);
        }
    };

    if let Some(host) = cli.host {
        config.server.host = host;
    }
    if let Some(port) = cli.port {
        config.server.port = port;
    }

    if config.discord.server.is_empty() || config.discord.channel.is_empty() {
        eprintln!(
            "Error: discord.server and discord.channel must be set in config.\n\
             Config file: {}",
            config::config_path().unwrap_or_default().display()
        );
        std::process::exit(1);
    }

    let db_path = config::expand_path(&config.database.path);
    let store = match db::MessageStore::new(&db_path) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln!("Failed to initialize database: {e}");
            std::process::exit(1);
        }
    };

    // Shared state for resync coordination
    let resync_notify = Arc::new(Notify::new());
    let monitor_status: Arc<RwLock<String>> = Arc::new(RwLock::new("starting".to_string()));

    info!(
        "Starting discord-channel-to-html: server={}, channel={}",
        config.discord.server, config.discord.channel
    );

    // Monitor runs in a restart loop: when resync_notify fires, the current
    // monitor run is cancelled and a fresh one starts from scratch.
    let monitor_store = store.clone();
    let monitor_config = config.clone();
    let monitor_resync = resync_notify.clone();
    let monitor_status_ref = monitor_status.clone();
    let monitor_handle = tokio::spawn(async move {
        loop {
            let mon = monitor::Monitor::new(
                monitor_config.clone(),
                monitor_store.clone(),
                monitor_status_ref.clone(),
            );
            tokio::select! {
                result = mon.run() => {
                    match result {
                        Ok(()) => info!("Monitor exited cleanly"),
                        Err(e) if e.is_wrong_location() => {
                            eprintln!("\nError: {e}\n");
                            std::process::exit(1);
                        }
                        Err(e) => error!("Monitor error: {e}"),
                    }
                    // Brief pause before restarting on unexpected exit
                    sleep(Duration::from_secs(5)).await;
                }
                _ = monitor_resync.notified() => {
                    info!("Resync triggered — restarting monitor");
                }
            }
        }
    });

    let server_store = store.clone();
    let server_config = config.clone();
    let server_handle = tokio::spawn(async move {
        if let Err(e) = server::serve(server_config, server_store, resync_notify, monitor_status).await {
            error!("Server error: {e}");
        }
    });

    tokio::select! {
        _ = monitor_handle => error!("Monitor exited unexpectedly"),
        _ = server_handle => error!("Server exited unexpectedly"),
        _ = tokio::signal::ctrl_c() => info!("Shutting down"),
    }
}
