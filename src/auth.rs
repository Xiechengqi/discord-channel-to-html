use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;

use crate::config::AuthConfig;

pub fn check_auth(headers: &HeaderMap, auth_config: &AuthConfig) -> bool {
    if auth_config.is_public() {
        return true;
    }

    if let Some(bearer) = extract_bearer(headers) {
        return bearer == auth_config.api_key;
    }

    false
}

fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(AUTHORIZATION)?.to_str().ok()?;
    raw.strip_prefix("Bearer ").map(ToString::to_string)
}
