use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::errors::{AppError, AppResult};

const CONFIG_DIR_NAME: &str = "discord-channel-to-html";
const CONFIG_FILE_NAME: &str = "config.toml";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordConfig {
    pub server: String,
    pub channel: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub api_key: String,
}

impl AuthConfig {
    pub fn is_public(&self) -> bool {
        self.api_key.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScraperConfig {
    pub poll_interval_secs: u64,
    pub initial_scroll_pages: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBrowserConfig {
    pub binary: String,
    pub session_name: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_timeout_secs() -> u64 {
    60
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub discord: DiscordConfig,
    pub server: ServerConfig,
    pub auth: AuthConfig,
    pub scraper: ScraperConfig,
    pub agent_browser: AgentBrowserConfig,
    pub database: DatabaseConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        let db_path = dirs::home_dir()
            .map(|h| {
                h.join(".config")
                    .join(CONFIG_DIR_NAME)
                    .join("messages.db")
                    .to_string_lossy()
                    .to_string()
            })
            .unwrap_or_else(|| "messages.db".to_string());

        Self {
            discord: DiscordConfig {
                server: String::new(),
                channel: String::new(),
            },
            server: ServerConfig {
                host: "0.0.0.0".to_string(),
                port: 12236,
            },
            auth: AuthConfig {
                api_key: String::new(),
            },
            scraper: ScraperConfig {
                poll_interval_secs: 5,
                initial_scroll_pages: 100,
            },
            agent_browser: AgentBrowserConfig {
                binary: "agent-browser".to_string(),
                session_name: "discord-channel-html".to_string(),
                timeout_secs: 60,
            },
            database: DatabaseConfig { path: db_path },
        }
    }
}

pub fn config_dir() -> AppResult<PathBuf> {
    let home = dirs::home_dir()
        .ok_or_else(|| AppError::ConfigReadFailed("home directory not found".to_string()))?;
    Ok(home.join(".config").join(CONFIG_DIR_NAME))
}

pub fn config_path() -> AppResult<PathBuf> {
    Ok(config_dir()?.join(CONFIG_FILE_NAME))
}

pub fn expand_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().to_string();
        }
    }
    path.to_string()
}

async fn detect_agent_browser_binary() -> String {
    match tokio::process::Command::new("which")
        .arg("agent-browser")
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if value.is_empty() {
                "agent-browser".to_string()
            } else {
                value
            }
        }
        _ => "agent-browser".to_string(),
    }
}

pub async fn load_or_init() -> AppResult<AppConfig> {
    let path = config_path()?;
    if fs::try_exists(&path)
        .await
        .map_err(|err| AppError::ConfigReadFailed(err.to_string()))?
    {
        let raw = fs::read_to_string(&path)
            .await
            .map_err(|err| AppError::ConfigReadFailed(err.to_string()))?;
        let config = toml::from_str::<AppConfig>(&raw)
            .map_err(|err| AppError::ConfigReadFailed(err.to_string()))?;
        return Ok(config);
    }

    let mut config = AppConfig::default();
    config.agent_browser.binary = detect_agent_browser_binary().await;
    save(&path, &config).await?;
    Ok(config)
}

pub async fn save(path: &Path, config: &AppConfig) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|err| AppError::ConfigWriteFailed(err.to_string()))?;
    }

    let temp_path = path.with_extension("toml.tmp");
    let content = toml::to_string_pretty(config)
        .map_err(|err| AppError::ConfigWriteFailed(err.to_string()))?;

    fs::write(&temp_path, content)
        .await
        .map_err(|err| AppError::ConfigWriteFailed(err.to_string()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = std::fs::Permissions::from_mode(0o600);
        fs::set_permissions(&temp_path, permissions)
            .await
            .map_err(|err| AppError::ConfigWriteFailed(err.to_string()))?;
    }

    fs::rename(&temp_path, path)
        .await
        .map_err(|err| AppError::ConfigWriteFailed(err.to_string()))?;

    Ok(())
}
