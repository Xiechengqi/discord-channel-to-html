use crate::agent_browser::types::AgentBrowserResponse;
use crate::errors::{AppError, AppResult};

pub fn parse_response(stdout: &[u8]) -> AppResult<AgentBrowserResponse> {
    serde_json::from_slice(stdout).map_err(|err| AppError::BrowserExecutionFailed(err.to_string()))
}
