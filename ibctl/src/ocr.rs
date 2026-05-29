//! OCR verification helpers.
//!
//! OCR is deliberately a fallback/verification path: Swing component walking is
//! still the primary automation channel, but OCR can validate dialogs whose text
//! is missing from the Swing tree.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use thiserror::Error;

use crate::agent_client::{AgentClient, WindowInfo};

#[derive(Debug, Error)]
pub enum OcrError {
    #[error("agent screenshot failed: {0}")]
    Agent(#[from] crate::agent_client::AgentError),
    #[error("failed to write screenshot: {0}")]
    Write(std::io::Error),
    #[error("failed to run tesseract: {0}")]
    Spawn(std::io::Error),
    #[error("tesseract exited with status {status}: {stderr}")]
    Tesseract { status: String, stderr: String },
    #[error("tesseract output was not UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrVerification {
    pub matched: bool,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OcrMode {
    Disabled,
    Auto,
    Always,
}

impl OcrMode {
    fn from_env() -> Self {
        match std::env::var("IBCTL_OCR_VERIFICATION")
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

pub fn verification_enabled() -> bool {
    !matches!(OcrMode::from_env(), OcrMode::Disabled)
}

pub fn strict_verification() -> bool {
    std::env::var("IBCTL_OCR_VERIFICATION_STRICT")
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

pub async fn verify_window_contains_any(
    client: &AgentClient,
    window: &WindowInfo,
    needles: &[&str],
) -> Result<Option<OcrVerification>, OcrError> {
    let mode = OcrMode::from_env();
    if mode == OcrMode::Disabled {
        return Ok(None);
    }
    if mode == OcrMode::Auto && !tesseract_available() {
        log::debug!("OCR verification skipped: tesseract is not available");
        return Ok(None);
    }

    let screenshot = client.capture_screenshot(window.id).await?;
    log::debug!(
        "OCR captured {} screenshot for window '{}' ({}x{})",
        screenshot.format,
        window.title,
        screenshot.width,
        screenshot.height
    );
    let image_path = write_temp_png(window.id.0, &screenshot.bytes).map_err(OcrError::Write)?;

    let output = Command::new("tesseract")
        .arg(&image_path)
        .arg("stdout")
        .arg("--psm")
        .arg("6")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(OcrError::Spawn);

    let _ = std::fs::remove_file(&image_path);
    let output = output?;
    if !output.status.success() {
        return Err(OcrError::Tesseract {
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    let text = String::from_utf8(output.stdout)?;
    let matched = text_matches_any(&text, needles);
    Ok(Some(OcrVerification { matched, text }))
}

fn tesseract_available() -> bool {
    Command::new("tesseract")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn write_temp_png(window_id: u64, bytes: &[u8]) -> std::io::Result<PathBuf> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "ibctl-ocr-{}-{}-{}.png",
        std::process::id(),
        window_id,
        nonce
    ));
    std::fs::write(&path, bytes)?;
    Ok(path)
}

fn text_matches_any(text: &str, needles: &[&str]) -> bool {
    let haystack = text.to_ascii_lowercase();
    needles
        .iter()
        .any(|needle| haystack.contains(&needle.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_matching_is_case_insensitive() {
        assert!(text_matches_any(
            "Second Factor Authentication",
            &["factor authentication"]
        ));
    }

    #[test]
    fn text_matching_rejects_absent_needles() {
        assert!(!text_matches_any("Login", &["security code"]));
    }
}
