//! AT-SPI accessibility fallback for component dumps.
//!
//! This is used only when the in-JVM Swing dump is unavailable or too sparse.

use std::process::{Command, Stdio};

use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum AtspiError {
    #[error("failed to run AT-SPI helper: {0}")]
    Spawn(std::io::Error),
    #[error("AT-SPI helper exited with status {status}: {stderr}")]
    Helper { status: String, stderr: String },
    #[error("AT-SPI helper returned invalid UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("AT-SPI helper returned invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AtspiMode {
    Disabled,
    Auto,
    Always,
}

impl AtspiMode {
    fn from_env() -> Self {
        match std::env::var("IBCTL_ATSPI_FALLBACK")
            .unwrap_or_else(|_| "auto".to_string())
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "0" | "false" | "no" | "off" | "disabled" => Self::Disabled,
            "1" | "true" | "yes" | "on" | "enabled" | "always" => Self::Always,
            _ => Self::Auto,
        }
    }
}

pub fn enabled() -> bool {
    !matches!(AtspiMode::from_env(), AtspiMode::Disabled)
}

pub fn should_try_for_dump(value: &Value) -> bool {
    let buttons = value
        .get("buttons")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let textfields = value
        .get("textfields")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    let labels = value
        .get("labels")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    buttons == 0 && textfields == 0 && labels == 0
}

pub async fn dump_window_by_title(title: String) -> Result<Option<Value>, AtspiError> {
    if !enabled() {
        return Ok(None);
    }

    let helper = std::env::var("IBCTL_ATSPI_DUMP_COMMAND")
        .unwrap_or_else(|_| "/opt/ibctl/atspi_dump.py".to_string());
    if AtspiMode::from_env() == AtspiMode::Auto && !std::path::Path::new(&helper).exists() {
        log::debug!("AT-SPI fallback skipped: helper not found at {}", helper);
        return Ok(None);
    }

    tokio::task::spawn_blocking(move || run_helper(&helper, &title))
        .await
        .map_err(|e| AtspiError::Helper {
            status: "join error".to_string(),
            stderr: e.to_string(),
        })?
}

fn run_helper(helper: &str, title: &str) -> Result<Option<Value>, AtspiError> {
    let output = Command::new(helper)
        .arg("--title")
        .arg(title)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(AtspiError::Spawn)?;

    if !output.status.success() {
        return Err(AtspiError::Helper {
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    let stdout = String::from_utf8(output.stdout)?;
    let value: Value = serde_json::from_str(&stdout)?;
    if value.get("error").is_some() {
        return Ok(None);
    }
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_dump_requests_fallback() {
        assert!(should_try_for_dump(&serde_json::json!({
            "buttons": [],
            "textfields": [],
            "labels": []
        })));
    }

    #[test]
    fn populated_dump_does_not_request_fallback() {
        assert!(!should_try_for_dump(&serde_json::json!({
            "buttons": [{"text": "Log In"}],
            "textfields": [],
            "labels": []
        })));
    }
}
