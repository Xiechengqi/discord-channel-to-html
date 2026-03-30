mod agent_browser;
mod auth;
mod config;
mod db;
mod embedded;
mod errors;
mod monitor;
mod scraper;
mod server;
mod server_store;

use std::sync::Arc;

use clap::Parser;
use tracing::{error, info};

#[derive(Parser)]
#[command(name = "discord-channel-to-html")]
#[command(about = "Monitor Discord server channels and serve messages as HTML + JSON API")]
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

    if config.discord.server_url.is_empty() {
        eprintln!(
            "Error: discord.server_url must be set in config.\n\
             Example: server_url = \"https://discord.com/channels/799672011265015819\"\n\
             Config file: {}",
            config::config_path().unwrap_or_default().display()
        );
        std::process::exit(1);
    }

    let data_dir = config::expand_path(&config.database.path);
    let server_store = match server_store::ServerStore::new(&data_dir) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln!("Failed to initialize database: {e}");
            std::process::exit(1);
        }
    };

    info!(
        "Starting discord-channel-to-html: server_url={}",
        config.discord.server_url
    );

    let server_handle = tokio::spawn(async move {
        if let Err(e) = server::serve(config, server_store).await {
            error!("Server error: {e}");
        }
    });

    tokio::select! {
        _ = server_handle => error!("Server exited unexpectedly"),
        _ = tokio::signal::ctrl_c() => info!("Shutting down"),
    }
}
