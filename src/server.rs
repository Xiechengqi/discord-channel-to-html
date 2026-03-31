use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{Notify, RwLock};

use crate::auth::check_auth;
use crate::config::AppConfig;
use crate::errors::AppError;
use crate::server_store::ServerStore;

pub struct AppState {
    pub server_store: Arc<ServerStore>,
    pub config: Arc<RwLock<AppConfig>>,
    pub start_time: Instant,
    pub monitor_notify: Arc<Notify>,
    pub pending_resyncs: Arc<Mutex<HashSet<String>>>,
}

pub async fn serve(
    config: AppConfig,
    server_store: Arc<ServerStore>,
) -> Result<(), Box<dyn std::error::Error>> {
    let monitor_notify = Arc::new(Notify::new());
    let shared_config = Arc::new(RwLock::new(config.clone()));
    let pending_resyncs: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));

    let state = Arc::new(AppState {
        server_store: server_store.clone(),
        config: shared_config.clone(),
        start_time: Instant::now(),
        monitor_notify: monitor_notify.clone(),
        pending_resyncs: pending_resyncs.clone(),
    });

    // Spawn the monitor manager
    let monitor_config = shared_config.clone();
    let monitor_store = server_store.clone();
    let monitor_notify_ref = monitor_notify.clone();
    let monitor_resyncs = pending_resyncs.clone();
    tokio::spawn(async move {
        crate::monitor::run_monitor_loop(
            monitor_config,
            monitor_store,
            monitor_notify_ref,
            monitor_resyncs,
        ).await;
    });

    let app = Router::new()
        .route("/api/channels", get(get_channels))
        .route("/api/channels/refresh", post(refresh_channels))
        .route("/api/channels/{channel_id}/monitor", post(add_monitor))
        .route("/api/channels/{channel_id}/monitor", delete(remove_monitor))
        .route("/api/channels/{channel_id}/resync", post(resync_channel))
        .route("/api/messages", get(get_messages))
        .route("/api/messages/latest", get(get_latest))
        .route("/api/config", get(get_config))
        .route("/api/config", post(update_config))
        .route("/health", get(health))
        .fallback(crate::embedded::serve_static)
        .with_state(state);

    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("HTTP server listening on {}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let uptime = state.start_time.elapsed().as_secs();
    let channels = state.server_store.get_all_channels().unwrap_or_default();
    let monitored: Vec<_> = channels.iter().filter(|c| c.monitored).collect();
    let total_messages: u64 = monitored.iter()
        .map(|c| state.server_store.channel_message_count(&c.channel_id))
        .sum();
    let config = state.config.read().await;

    Json(json!({
        "ok": true,
        "server_url": config.discord.server_url,
        "uptime_secs": uptime,
        "total_channels": channels.len(),
        "monitored_channels": monitored.len(),
        "total_messages": total_messages,
    }))
}

/// Return all channels (from server.db).
async fn get_channels(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, AppError> {
    let channels = state.server_store.get_all_channels()?;
    let enriched: Vec<_> = channels.iter().map(|c| {
        let count = state.server_store.channel_message_count(&c.channel_id);
        json!({
            "channel_id": c.channel_id,
            "name": c.name,
            "type": c.channel_type,
            "channel_url": c.channel_url,
            "monitored": c.monitored,
            "message_count": count,
        })
    }).collect();

    Ok(Json(json!({
        "ok": true,
        "channels": enriched,
    })))
}

/// Re-read channel list from Discord DOM.
async fn refresh_channels(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    let config = state.config.read().await;
    if !check_auth(&headers, &config.auth) {
        return Err(AppError::AuthRequired);
    }

    use crate::agent_browser::client::AgentBrowserClient;
    use crate::agent_browser::types::AgentBrowserOptions;

    let client = AgentBrowserClient::new(AgentBrowserOptions {
        binary: config.agent_browser.binary.clone(),
        session_name: config.agent_browser.session_name.clone(),
        timeout_secs: config.agent_browser.timeout_secs,
    });

    let guild_id = config.discord.guild_id();
    client.open(&config.discord.server_url).await?;
    client.wait_ms(2000).await?;

    let channels = crate::scraper::list_channels(
        &client,
        &guild_id,
        &config.discord.server_url,
    ).await?;

    state.server_store.upsert_channels(&channels)?;

    let all = state.server_store.get_all_channels()?;
    Ok((StatusCode::OK, Json(json!({
        "ok": true,
        "channels": all,
    }))))
}

/// Add a channel to monitoring.
async fn add_monitor(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(channel_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    if !check_auth(&headers, &state.config.read().await.auth) {
        return Err(AppError::AuthRequired);
    }
    state.server_store.add_monitored(&channel_id)?;
    state.monitor_notify.notify_one();
    Ok((StatusCode::OK, Json(json!({ "ok": true }))))
}

/// Remove a channel from monitoring (keeps data).
async fn remove_monitor(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(channel_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    if !check_auth(&headers, &state.config.read().await.auth) {
        return Err(AppError::AuthRequired);
    }
    state.server_store.remove_monitored(&channel_id)?;
    state.monitor_notify.notify_one();
    Ok((StatusCode::OK, Json(json!({ "ok": true }))))
}

/// Resync a specific channel (clear data + re-scrape).
async fn resync_channel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(channel_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    if !check_auth(&headers, &state.config.read().await.auth) {
        return Err(AppError::AuthRequired);
    }
    state.server_store.clear_channel_data(&channel_id)?;
    if let Ok(mut pending) = state.pending_resyncs.lock() {
        pending.insert(channel_id.clone());
    }
    state.monitor_notify.notify_one();
    Ok((StatusCode::OK, Json(json!({ "ok": true }))))
}

#[derive(Deserialize)]
struct MessagesQuery {
    channel_id: Option<String>,
    before: Option<String>,
    after: Option<String>,
    limit: Option<u32>,
    before_id: Option<i64>,
}

async fn get_messages(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<MessagesQuery>,
) -> Result<impl IntoResponse, AppError> {
    if !check_auth(&headers, &state.config.read().await.auth) {
        return Err(AppError::AuthRequired);
    }

    let channel_id = query.channel_id.as_deref().unwrap_or("");
    if channel_id.is_empty() {
        return Err(AppError::InvalidParams("channel_id is required".to_string()));
    }

    let store = state.server_store.get_message_store(channel_id)?;
    let limit = query.limit.unwrap_or(50).min(200);

    let messages = if let Some(before_id) = query.before_id {
        store.get_before_id(before_id, limit)?
    } else {
        store.get_messages(
            query.before.as_deref(),
            query.after.as_deref(),
            limit,
        )?
    };

    let total = store.count()?;

    Ok(Json(json!({
        "ok": true,
        "messages": messages,
        "count": messages.len(),
        "total": total,
        "has_more": messages.len() as u32 == limit,
    })))
}

#[derive(Deserialize)]
struct LatestQuery {
    channel_id: Option<String>,
    n: Option<u32>,
}

async fn get_latest(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<LatestQuery>,
) -> Result<impl IntoResponse, AppError> {
    if !check_auth(&headers, &state.config.read().await.auth) {
        return Err(AppError::AuthRequired);
    }

    let channel_id = query.channel_id.as_deref().unwrap_or("");
    if channel_id.is_empty() {
        return Err(AppError::InvalidParams("channel_id is required".to_string()));
    }

    let store = state.server_store.get_message_store(channel_id)?;
    let n = query.n.unwrap_or(20).min(500);
    let messages = store.get_latest(n)?;
    let total = store.count()?;

    Ok(Json(json!({
        "ok": true,
        "messages": messages,
        "count": messages.len(),
        "total": total,
    })))
}

async fn get_config(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let config = state.config.read().await;
    Json(json!({
        "ok": true,
        "poll_interval_secs": config.scraper.poll_interval_secs,
        "max_history_pages": config.scraper.max_history_pages,
    }))
}

#[derive(Deserialize)]
struct ConfigUpdate {
    poll_interval_secs: Option<u64>,
    max_history_pages: Option<Option<u64>>,
}

async fn update_config(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(update): Json<ConfigUpdate>,
) -> Result<impl IntoResponse, AppError> {
    // Build updated config from a snapshot (don't hold write lock during I/O)
    let mut new_config = state.config.read().await.clone();
    if !check_auth(&headers, &new_config.auth) {
        return Err(AppError::AuthRequired);
    }

    if let Some(interval) = update.poll_interval_secs {
        new_config.scraper.poll_interval_secs = interval;
    }
    if let Some(pages) = update.max_history_pages {
        new_config.scraper.max_history_pages = pages;
    }

    // Save to disk first — only update memory if persist succeeds
    let path = crate::config::config_path()?;
    crate::config::save(&path, &new_config).await?;

    *state.config.write().await = new_config;

    // Wake monitor loop so new poll_interval takes effect immediately
    state.monitor_notify.notify_one();

    Ok((StatusCode::OK, Json(json!({ "ok": true }))))
}
