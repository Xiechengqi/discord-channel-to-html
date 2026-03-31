use std::fmt::{Display, Formatter};

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    AuthRequired,
    InvalidParams,
    BrowserNotFound,
    BrowserExecutionFailed,
    ConfigReadFailed,
    ConfigWriteFailed,
    DatabaseError,
    InternalError,
}

impl Display for ErrorCode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::AuthRequired => "AUTH_REQUIRED",
            Self::InvalidParams => "INVALID_PARAMS",
            Self::BrowserNotFound => "BROWSER_NOT_FOUND",
            Self::BrowserExecutionFailed => "BROWSER_EXECUTION_FAILED",
            Self::ConfigReadFailed => "CONFIG_READ_FAILED",
            Self::ConfigWriteFailed => "CONFIG_WRITE_FAILED",
            Self::DatabaseError => "DATABASE_ERROR",
            Self::InternalError => "INTERNAL_ERROR",
        };
        write!(f, "{value}")
    }
}

#[derive(Debug, Error)]
pub enum AppError {
    #[error("authentication required")]
    AuthRequired,
    #[error("invalid parameters: {0}")]
    InvalidParams(String),
    #[error("agent-browser binary not found")]
    BrowserNotFound,
    #[error("agent-browser execution failed: {0}")]
    BrowserExecutionFailed(String),
    #[error("failed to read config: {0}")]
    ConfigReadFailed(String),
    #[error("failed to write config: {0}")]
    ConfigWriteFailed(String),
    #[error("database error: {0}")]
    DatabaseError(String),
    #[allow(dead_code)]
    #[error("{0}")]
    Internal(String),
    /// Browser is not on the expected Discord server/channel — requires human intervention.
    #[error("wrong location: {0}")]
    WrongLocation(String),
}

impl AppError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::AuthRequired => ErrorCode::AuthRequired,
            Self::InvalidParams(_) => ErrorCode::InvalidParams,
            Self::BrowserNotFound => ErrorCode::BrowserNotFound,
            Self::BrowserExecutionFailed(_) => ErrorCode::BrowserExecutionFailed,
            Self::ConfigReadFailed(_) => ErrorCode::ConfigReadFailed,
            Self::ConfigWriteFailed(_) => ErrorCode::ConfigWriteFailed,
            Self::DatabaseError(_) => ErrorCode::DatabaseError,
            Self::Internal(_) | Self::WrongLocation(_) => ErrorCode::InternalError,
        }
    }

    pub fn status_code(&self) -> StatusCode {
        match self {
            Self::AuthRequired => StatusCode::UNAUTHORIZED,
            Self::InvalidParams(_) => StatusCode::BAD_REQUEST,
            Self::BrowserNotFound => StatusCode::SERVICE_UNAVAILABLE,
            Self::BrowserExecutionFailed(_)
            | Self::ConfigReadFailed(_)
            | Self::ConfigWriteFailed(_)
            | Self::DatabaseError(_)
            | Self::Internal(_)
            | Self::WrongLocation(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let body = Json(serde_json::json!({
            "ok": false,
            "error": {
                "code": self.code(),
                "message": self.to_string(),
            }
        }));
        (self.status_code(), body).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
