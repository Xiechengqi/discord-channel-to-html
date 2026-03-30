use std::sync::{Arc, RwLock};
use std::time::Instant;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Notify;

use crate::auth::check_auth;
use crate::config::AppConfig;
use crate::db::MessageStore;
use crate::errors::AppError;

pub struct AppState {
    pub store: Arc<MessageStore>,
    pub config: AppConfig,
    pub start_time: Instant,
    pub resync_notify: Arc<Notify>,
    pub monitor_status: Arc<RwLock<String>>,
}

pub async fn serve(
    config: AppConfig,
    store: Arc<MessageStore>,
    resync_notify: Arc<Notify>,
    monitor_status: Arc<RwLock<String>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let state = Arc::new(AppState {
        store,
        config: config.clone(),
        start_time: Instant::now(),
        resync_notify,
        monitor_status,
    });

    let app = Router::new()
        .route("/api/messages", get(get_messages))
        .route("/api/messages/latest", get(get_latest))
        .route("/api/resync", post(resync))
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
    let count = state.store.count().unwrap_or(0);
    let uptime = state.start_time.elapsed().as_secs();
    let status = state.monitor_status.read()
        .map(|s| s.clone())
        .unwrap_or_else(|_| "unknown".to_string());

    Json(json!({
        "ok": true,
        "message_count": count,
        "uptime_secs": uptime,
        "channel": state.config.discord.channel_url,
        "monitor_status": status,
    }))
}

async fn resync(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, AppError> {
    if !check_auth(&headers, &state.config.auth) {
        return Err(AppError::AuthRequired);
    }

    state.store.clear()?;
    if let Ok(mut s) = state.monitor_status.write() {
        *s = "resyncing".to_string();
    }
    state.resync_notify.notify_one();

    Ok((StatusCode::OK, Json(json!({ "ok": true }))))
}

#[derive(Deserialize)]
struct MessagesQuery {
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
    if !check_auth(&headers, &state.config.auth) {
        return Err(AppError::AuthRequired);
    }

    let limit = query.limit.unwrap_or(50).min(200);

    let messages = if let Some(before_id) = query.before_id {
        state.store.get_before_id(before_id, limit)?
    } else {
        state.store.get_messages(
            query.before.as_deref(),
            query.after.as_deref(),
            limit,
        )?
    };

    let total = state.store.count()?;

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
    n: Option<u32>,
}

async fn get_latest(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<LatestQuery>,
) -> Result<impl IntoResponse, AppError> {
    if !check_auth(&headers, &state.config.auth) {
        return Err(AppError::AuthRequired);
    }

    let n = query.n.unwrap_or(20).min(500);
    let messages = state.store.get_latest(n)?;
    let total = state.store.count()?;

    Ok(Json(json!({
        "ok": true,
        "messages": messages,
        "count": messages.len(),
        "total": total,
    })))
}
