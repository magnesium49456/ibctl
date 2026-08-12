//! Two-factor authentication dialog handler.
//!
//! Recognizes the TOTP/2FA challenge dialog and submits a generated code.

use std::future::Future;
use std::pin::Pin;

use secrecy::{ExposeSecret, SecretString};

use crate::agent_client::{AgentClient, WindowInfo};
use crate::agent_events::is_twofa_title;
use crate::config::TotpProvider;
use crate::handlers::{DialogHandler, HandlerError, HandlerResult};
use crate::totp;

const TWOFA_OCR_NEEDLES: &[&str] = &[
    "second factor",
    "security code",
    "authentication code",
    "mobile authentication",
    "enter the code",
    "verification code",
    "one-time password",
    "passcode",
];

pub fn looks_like_twofa_components(components: &serde_json::Value) -> bool {
    let field_count = components
        .get("textfields")
        .and_then(serde_json::Value::as_array)
        .map(|fields| {
            fields
                .iter()
                .filter(|field| {
                    field
                        .get("visible")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(true)
                        && field
                            .get("enabled")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(true)
                        && field
                            .get("editable")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(true)
                })
                .count()
        })
        .unwrap_or(0);
    if field_count == 0 {
        return false;
    }

    let mut strings = Vec::new();
    collect_semantic_strings(components, None, &mut strings);
    let text = strings.join(" ").to_ascii_lowercase();
    let has_code_semantics = [
        "security code",
        "authentication code",
        "verification code",
        "one-time code",
        "one time code",
        "one-time password",
        "passcode",
        "enter the code",
        "verification",
        " otp ",
    ]
    .iter()
    .any(|needle| text.contains(needle));
    let looks_like_credentials = ["username", "user name", "account username"]
        .iter()
        .any(|needle| text.contains(needle))
        && text.contains("password");

    has_code_semantics && !looks_like_credentials
}

fn collect_semantic_strings<'a>(
    value: &'a serde_json::Value,
    key: Option<&str>,
    output: &mut Vec<&'a str>,
) {
    match value {
        serde_json::Value::String(text) => {
            // Do not inspect or retain entered field values. Component metadata,
            // labels, roles, and button text are sufficient for classification.
            if key != Some("text") && !text.trim().is_empty() {
                output.push(text);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_semantic_strings(item, key, output);
            }
        }
        serde_json::Value::Object(map) => {
            for (child_key, child) in map {
                collect_semantic_strings(child, Some(child_key), output);
            }
        }
        _ => {}
    }
}

/// Handles the second factor authentication dialog by generating a TOTP
/// code and entering it.
pub struct TotpEntryHandler {
    /// Name of the env var holding the TOTP secret
    secret_env: String,
    /// TOTP provider type
    provider: TotpProvider,
}

impl TotpEntryHandler {
    pub fn new(secret_env: String, provider: TotpProvider) -> Self {
        Self {
            secret_env,
            provider,
        }
    }
}

impl DialogHandler for TotpEntryHandler {
    fn name(&self) -> &str {
        "TotpEntryHandler"
    }

    fn can_handle(&self, window: &WindowInfo) -> bool {
        is_twofa_title(&window.title)
    }

    fn handle<'a>(
        &'a self,
        client: &'a AgentClient,
        window: &'a WindowInfo,
    ) -> Pin<Box<dyn Future<Output = Result<HandlerResult, HandlerError>> + Send + 'a>> {
        Box::pin(async move {
            log::info!("Handling 2FA dialog '{}'", window.title);

            if crate::ocr::verification_enabled() {
                match crate::ocr::verify_window_contains_any(client, window, TWOFA_OCR_NEEDLES)
                    .await
                {
                    Ok(Some(result)) if result.matched => {
                        log::info!("OCR verified 2FA dialog text before code entry");
                    }
                    Ok(Some(result)) => {
                        let message = format!(
                            "OCR did not recognize expected 2FA text in '{}'",
                            window.title
                        );
                        if crate::ocr::strict_verification() {
                            return Ok(HandlerResult::Error(message));
                        }
                        log::warn!("{}; continuing because strict OCR is disabled", message);
                        log::debug!("OCR text was: {}", result.text.trim());
                    }
                    Ok(None) => {
                        log::debug!("OCR verification skipped for '{}'", window.title);
                    }
                    Err(e) => {
                        if crate::ocr::strict_verification() {
                            return Ok(HandlerResult::Error(format!(
                                "strict OCR verification failed: {}",
                                e
                            )));
                        }
                        log::warn!("OCR verification failed: {}; continuing", e);
                    }
                }
            }

            // Read the TOTP secret from the configured env var (wrapped in SecretString)
            let secret = SecretString::from(std::env::var(&self.secret_env).map_err(|_| {
                HandlerError::Failed {
                    handler: self.name().to_string(),
                    reason: format!("TOTP secret env var '{}' not set", self.secret_env),
                }
            })?);

            // Generate TOTP code in a blocking task to avoid blocking the runtime
            totp::wait_for_safe_window().await;
            let corrected_timestamp = crate::time_sync::corrected_unix_seconds()
                .map_err(|e| HandlerError::Failed {
                    handler: self.name().to_string(),
                    reason: format!("failed to read corrected clock: {e}"),
                })?;
            let provider_type = self.provider;
            let handler_name = self.name().to_string();
            let totp_code = tokio::task::spawn_blocking(move || {
                let provider = totp::create_provider(provider_type)?;
                provider.generate_at(secret.expose_secret(), corrected_timestamp)
            })
            .await
            .map_err(|e| HandlerError::Failed {
                handler: handler_name.clone(),
                reason: format!("TOTP task panicked: {}", e),
            })?
            .map_err(|e| HandlerError::Failed {
                handler: handler_name,
                reason: format!("failed to generate TOTP code: {}", e),
            })?;

            // Consume the single-use TotpCode and use semantic field selection.
            // The Java agent scores labels/accessibility metadata/focus and falls
            // back to AT-SPI when a Gateway update changes the Swing component tree.
            let code = totp_code.into_inner();
            let typed = client
                .type_text_best(window.id, &code)
                .await
                .map_err(HandlerError::AgentError)?;
            if !typed {
                return Ok(HandlerResult::Error(
                    "2FA code field not found or not writable".into(),
                ));
            }

            // Prefer an explicit submit button. IBKR has renamed this control
            // across releases, so try semantic variants before pressing Enter.
            let mut submitted = false;
            for label in ["OK", "Verify", "Submit", "Continue", "Next"] {
                match client.click_button(window.id, label).await {
                    Ok(true) => {
                        log::debug!("Submitted 2FA code via '{}' button", label);
                        submitted = true;
                        break;
                    }
                    Ok(false) => {}
                    Err(error) => {
                        log::debug!("2FA submit button '{}' was unavailable: {}", label, error);
                    }
                }
            }

            if !submitted {
                log::debug!("No 2FA submit button found — falling back to Enter");
                client
                    .send_key(window.id, "Enter")
                    .await
                    .map_err(HandlerError::AgentError)?;
            }

            log::info!("2FA code submitted");
            Ok(HandlerResult::Handled)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::WindowId;

    #[test]
    fn test_can_handle_second_factor_window() {
        let handler = TotpEntryHandler::new("TWOFACTOR_CODE".to_string(), TotpProvider::Oathtool);
        let window = WindowInfo {
            id: WindowId(1),
            title: "Second Factor Authentication".to_string(),
            class: "dialog".to_string(),
            bounds: None,
            visible: true,
        };

        assert!(handler.can_handle(&window));
    }

    #[test]
    fn test_can_handle_security_code_window() {
        let handler = TotpEntryHandler::new("TWOFACTOR_CODE".to_string(), TotpProvider::Oathtool);
        let window = WindowInfo {
            id: WindowId(1),
            title: "Security Code".to_string(),
            class: "dialog".to_string(),
            bounds: None,
            visible: true,
        };

        assert!(handler.can_handle(&window));
    }

    #[test]
    fn test_can_handle_ibkr_mobile_authentication_window() {
        let handler = TotpEntryHandler::new("TWOFACTOR_CODE".to_string(), TotpProvider::Oathtool);
        let window = WindowInfo {
            id: WindowId(1),
            title: "IBKR Mobile Authentication".to_string(),
            class: "dialog".to_string(),
            bounds: None,
            visible: true,
        };

        assert!(handler.can_handle(&window));
    }

    #[test]
    fn detects_twofa_from_component_semantics_when_title_changes() {
        let components = serde_json::json!({
            "labels": ["Enter the verification code from your authenticator"],
            "textfields": [{
                "index": 2,
                "visible": true,
                "enabled": true,
                "editable": true,
                "metadata": "Verification code"
            }],
            "buttons": [{"text": "Continue"}]
        });

        assert!(looks_like_twofa_components(&components));
    }

    #[test]
    fn does_not_mistake_login_form_for_twofa() {
        let components = serde_json::json!({
            "labels": ["Username", "Password"],
            "textfields": [
                {"visible": true, "enabled": true, "editable": true, "metadata": "Username"},
                {"visible": true, "enabled": true, "editable": true, "metadata": "Password"}
            ],
            "buttons": [{"text": "Log In"}]
        });

        assert!(!looks_like_twofa_components(&components));
    }
}
