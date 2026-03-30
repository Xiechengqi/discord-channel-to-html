use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::process::Command;

use crate::agent_browser::parser::parse_response;
use crate::agent_browser::types::{
    AgentBrowserEvalResult, AgentBrowserOptions, AgentBrowserResponse,
};
use crate::errors::{AppError, AppResult};

#[derive(Clone)]
pub struct AgentBrowserClient {
    options: AgentBrowserOptions,
}

impl AgentBrowserClient {
    pub fn new(options: AgentBrowserOptions) -> Self {
        Self { options }
    }

    pub async fn run(&self, args: &[&str]) -> AppResult<AgentBrowserResponse> {
        let mut command = Command::new(&self.options.binary);
        command.arg("--json");
        command
            .arg("--session-name")
            .arg(&self.options.session_name);
        command.args(args);

        let deadline = Duration::from_secs(self.options.timeout_secs);
        let output = tokio::time::timeout(deadline, command.output())
            .await
            .map_err(|_| {
                AppError::BrowserExecutionFailed(format!(
                    "agent-browser timed out after {}s",
                    self.options.timeout_secs
                ))
            })?
            .map_err(|err| AppError::BrowserExecutionFailed(err.to_string()))?;

        if !output.status.success() {
            if let Ok(response) = serde_json::from_slice::<AgentBrowserResponse>(&output.stdout) {
                if !response.success {
                    let msg = response.error.unwrap_or_default();
                    if !msg.is_empty() {
                        return Err(AppError::BrowserExecutionFailed(msg));
                    }
                }
            }
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let detail = if !stderr.is_empty() {
                stderr
            } else if !stdout.is_empty() {
                stdout
            } else {
                format!("exit code {}", output.status.code().unwrap_or(-1))
            };
            return Err(if detail.contains("No such file") {
                AppError::BrowserNotFound
            } else {
                AppError::BrowserExecutionFailed(detail)
            });
        }

        parse_response(&output.stdout)
    }

    pub async fn open(&self, url: &str) -> AppResult<()> {
        let response = self.run(&["open", url]).await?;
        if response.success {
            Ok(())
        } else {
            Err(AppError::BrowserExecutionFailed(
                response
                    .error
                    .unwrap_or_else(|| "agent-browser open failed".to_string()),
            ))
        }
    }

    pub async fn eval(&self, script: &str) -> AppResult<AgentBrowserEvalResult> {
        let response = self.run(&["eval", script]).await?;
        if !response.success {
            return Err(AppError::BrowserExecutionFailed(
                response
                    .error
                    .unwrap_or_else(|| "agent-browser eval failed".to_string()),
            ));
        }

        let data = response.data.unwrap_or(Value::Null);
        let result = data.get("result").cloned().unwrap_or(Value::Null);
        Ok(AgentBrowserEvalResult { result })
    }

    pub async fn eval_json<T>(&self, script: &str) -> AppResult<T>
    where
        T: DeserializeOwned,
    {
        let evaluated = self.eval(script).await?;
        if let Some(value) = evaluated.result.as_str() {
            serde_json::from_str::<T>(value)
                .map_err(|err| AppError::BrowserExecutionFailed(err.to_string()))
        } else {
            serde_json::from_value::<T>(evaluated.result)
                .map_err(|err| AppError::BrowserExecutionFailed(err.to_string()))
        }
    }

    pub async fn wait_ms(&self, milliseconds: u64) -> AppResult<()> {
        let response = self.run(&["wait", &milliseconds.to_string()]).await?;
        if response.success {
            Ok(())
        } else {
            Err(AppError::BrowserExecutionFailed(
                response
                    .error
                    .unwrap_or_else(|| "agent-browser wait failed".to_string()),
            ))
        }
    }
}
