//! AT-SPI accessibility fallback for component dumps.
//!
//! This is used only when the in-JVM Swing dump is unavailable or too sparse.

use std::io::Write;
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
    #[error("failed to write text to AT-SPI helper stdin: {0}")]
    Stdin(std::io::Error),
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
    let mut semantic_text = String::new();
    collect_strings(value.get("labels"), &mut semantic_text);
    collect_strings(value.get("textareas"), &mut semantic_text);
    let semantic_text = semantic_text.to_ascii_lowercase();
    let authentication_without_field = textfields == 0
        && [
            "security code",
            "authentication code",
            "verification code",
            "one-time",
            "passcode",
            "authenticator",
        ]
        .iter()
        .any(|needle| semantic_text.contains(needle));

    (buttons == 0 && textfields == 0 && labels == 0) || authentication_without_field
}

fn collect_strings(value: Option<&Value>, output: &mut String) {
    match value {
        Some(Value::String(text)) => {
            output.push(' ');
            output.push_str(text);
        }
        Some(Value::Array(items)) => {
            for item in items {
                collect_strings(Some(item), output);
            }
        }
        Some(Value::Object(map)) => {
            for item in map.values() {
                collect_strings(Some(item), output);
            }
        }
        _ => {}
    }
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

pub async fn type_text_by_title(title: String, value: String) -> Result<bool, AtspiError> {
    if !enabled() {
        return Ok(false);
    }

    let helper = std::env::var("IBCTL_ATSPI_DUMP_COMMAND")
        .unwrap_or_else(|_| "/opt/ibctl/atspi_dump.py".to_string());
    if AtspiMode::from_env() == AtspiMode::Auto && !std::path::Path::new(&helper).exists() {
        log::debug!("AT-SPI text entry skipped: helper not found at {}", helper);
        return Ok(false);
    }

    tokio::task::spawn_blocking(move || run_type_helper(&helper, &title, &value))
        .await
        .map_err(|error| AtspiError::Helper {
            status: "join error".to_string(),
            stderr: error.to_string(),
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

fn run_type_helper(helper: &str, title: &str, value: &str) -> Result<bool, AtspiError> {
    const HINTS: &str =
        "security code|authentication code|verification code|one-time code|passcode|otp|code";
    let mut child = Command::new(helper)
        .arg("--title")
        .arg(title)
        .arg("--type-text-stdin")
        .arg("--hints")
        .arg(HINTS)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(AtspiError::Spawn)?;

    child
        .stdin
        .take()
        .ok_or_else(|| {
            AtspiError::Stdin(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "AT-SPI helper stdin was not available",
            ))
        })?
        .write_all(value.as_bytes())
        .map_err(AtspiError::Stdin)?;

    let output = child.wait_with_output().map_err(AtspiError::Spawn)?;
    let stdout = String::from_utf8(output.stdout)?;
    if let Ok(response) = serde_json::from_str::<Value>(&stdout) {
        if let Some(typed) = response.get("typed").and_then(Value::as_bool) {
            return Ok(typed);
        }
    }
    if !output.status.success() {
        return Err(AtspiError::Helper {
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    let response: Value = serde_json::from_str(&stdout)?;
    Ok(response
        .get("typed")
        .and_then(Value::as_bool)
        .unwrap_or(false))
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

    #[test]
    fn authentication_dump_without_textfield_requests_fallback() {
        assert!(should_try_for_dump(&serde_json::json!({
            "buttons": [{"text": "Continue"}],
            "textfields": [],
            "labels": ["Enter your verification code"]
        })));
    }
}
