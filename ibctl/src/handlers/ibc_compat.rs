//! Additional IBC-style dialog compatibility handlers.
//!
//! The dedicated handlers cover the core login/API flow. This module handles
//! the long tail of IBC dialog classes using conservative title + component-text
//! matching, with order-transmission decisions left manual unless explicitly
//! enabled by env vars.

use std::future::Future;
use std::pin::Pin;

use serde_json::Value;

use crate::agent_client::{AgentClient, WindowInfo};
use crate::handlers::{DialogHandler, HandlerError, HandlerResult};

pub const IBC_DIALOG_COVERAGE: &[&str] = &[
    "AcceptIncomingConnectionDialogHandler",
    "ApiChangeConfirmationDialogHandler",
    "AutoRestartConfirmationDialog",
    "BidAskLastSizeDisplayUpdateDialogHandler",
    "BlindTradingWarningDialogHandler",
    "CryptoOrderConfirmationDialogHandler",
    "ExistingSessionDetectedDialogHandler",
    "GatewayLoginFrameHandler",
    "LoginErrorDialogHandler",
    "LoginFailedDialogHandler",
    "NewerVersionDialogHandler",
    "NonBrokerageAccountDialogHandler",
    "NotCurrentlyAvailableDialogHandler",
    "PasswordExpiryWarningFrameHandler",
    "ReLoginDialogHandler",
    "SecondFactorAuthenticationDialogHandler",
    "SecurityCodeDialogHandler",
    "SslReconnectDialogHandler",
    "TipOfTheDayDialogHandler",
    "TooManyFailedLoginAttemptsDialogHandler",
    "TradingLoginHandoffDialogHandler",
    "ApiPrecautionWarningDialog",
    "AutoLogoffRestartConfirmationDialog",
    "ConnectionLostDialogHandler",
    "GatewayNotificationDialogHandler",
    "OrderPreviewDialogHandler",
    "ReadOnlyApiWarningDialog",
    "VersionNoticeDialogHandler",
];

pub struct IbcCompatibilityDialogHandler;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DialogAction {
    name: &'static str,
    buttons: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CryptoOrderDecision {
    Manual,
    Transmit,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DialogPolicy {
    dismiss_password_expiry: bool,
    accept_bid_ask_last_size_update: bool,
    crypto_order_decision: CryptoOrderDecision,
    allow_blind_trading: bool,
    bypass_order_precautions: bool,
}

impl DialogPolicy {
    fn from_env() -> Self {
        let crypto_order_decision = match env_lower("CONFIRM_CRYPTO_CURRENCY_ORDERS").as_deref() {
            Some("transmit") => CryptoOrderDecision::Transmit,
            Some("cancel") => CryptoOrderDecision::Cancel,
            _ => CryptoOrderDecision::Manual,
        };

        Self {
            dismiss_password_expiry: env_yes("DISMISS_PASSWORD_EXPIRY_WARNING"),
            accept_bid_ask_last_size_update: matches!(
                env_lower("ACCEPT_BID_ASK_LAST_SIZE_DISPLAY_UPDATE_NOTIFICATION").as_deref(),
                Some("accept" | "defer")
            ),
            crypto_order_decision,
            allow_blind_trading: env_yes("ALLOW_BLIND_TRADING"),
            bypass_order_precautions: env_yes("BYPASS_WARNING")
                || env_yes("BYPASS_ORDER_PRECAUTIONS"),
        }
    }
}

impl Default for DialogPolicy {
    fn default() -> Self {
        Self {
            dismiss_password_expiry: false,
            accept_bid_ask_last_size_update: false,
            crypto_order_decision: CryptoOrderDecision::Manual,
            allow_blind_trading: false,
            bypass_order_precautions: false,
        }
    }
}

impl DialogHandler for IbcCompatibilityDialogHandler {
    fn name(&self) -> &str {
        "IbcCompatibilityDialogHandler"
    }

    fn can_handle(&self, window: &WindowInfo) -> bool {
        let title = window.title.to_lowercase();
        if title.contains("configuration") {
            return false;
        }
        if is_large_gateway_window(window) {
            return false;
        }
        if (title.contains("ib gateway") || title.contains("ibkr gateway"))
            && window
                .bounds
                .as_ref()
                .is_some_and(|bounds| bounds.width < 650 && bounds.height < 400)
        {
            return true;
        }

        [
            "confirmation",
            "cryptocurrency order",
            "login",
            "newer version",
            "notice",
            "order preview",
            "password notice",
            "restart",
            "warning",
        ]
        .iter()
        .any(|needle| title.contains(needle))
    }

    fn handle<'a>(
        &'a self,
        client: &'a AgentClient,
        window: &'a WindowInfo,
    ) -> Pin<Box<dyn Future<Output = Result<HandlerResult, HandlerError>> + Send + 'a>> {
        Box::pin(async move {
            let components = client.dump_components(window.id).await.unwrap_or_else(|e| {
                log::debug!(
                    "Could not dump components for possible IBC dialog '{}': {}",
                    window.title,
                    e
                );
                Value::Null
            });
            let text = dialog_text_blob(window, &components);

            let Some(action) = classify_dialog(&window.title, &text) else {
                return Ok(HandlerResult::NotApplicable);
            };

            log::info!(
                "Handling IBC-compatible dialog '{}' as {}",
                window.title,
                action.name
            );

            for label in action.buttons {
                match client.click_button(window.id, label).await {
                    Ok(true) => {
                        log::info!("{} dismissed via '{}'", action.name, label);
                        return Ok(HandlerResult::Handled);
                    }
                    Ok(false) => {}
                    Err(e) => log::debug!("{} button '{}' failed: {}", action.name, label, e),
                }
            }

            Ok(HandlerResult::Error(format!(
                "{} matched but no expected button was found",
                action.name
            )))
        })
    }
}

fn is_large_gateway_window(window: &WindowInfo) -> bool {
    let title = window.title.to_lowercase();
    if !title.contains("ib gateway") && !title.contains("ibkr gateway") {
        return false;
    }
    window
        .bounds
        .as_ref()
        .is_some_and(|bounds| bounds.width >= 650 && bounds.height >= 400)
}

fn classify_dialog(title: &str, text_blob: &str) -> Option<DialogAction> {
    classify_dialog_with_policy(title, text_blob, &DialogPolicy::from_env())
}

fn classify_dialog_with_policy(
    title: &str,
    text_blob: &str,
    policy: &DialogPolicy,
) -> Option<DialogAction> {
    let title = title.to_lowercase();
    let text = text_blob.to_lowercase();

    if text.contains("apply the new socket port setting") {
        return Some(action("ApiChangeConfirmationDialogHandler", &["Yes", "OK"]));
    }
    if text.contains("trading platform restart automatically") {
        return Some(action("AutoRestartConfirmationDialog", &["OK"]));
    }
    if (title.contains("restart confirmation")
        || text.contains("auto-restart")
        || text.contains("auto logoff"))
        && (text.contains("restart") || text.contains("logoff"))
    {
        return Some(action("AutoLogoffRestartConfirmationDialog", &["Yes", "OK"]));
    }
    if title.contains("newer version") || text.contains("newer version") {
        return Some(action("NewerVersionDialogHandler", &["OK", "No"]));
    }
    if title.contains("login failed") {
        return Some(action("LoginFailedDialogHandler", &["OK"]));
    }
    if title.contains("login error") {
        return Some(action("LoginErrorDialogHandler", &["OK"]));
    }
    if title.contains("login") && text.contains("not currently available") {
        return Some(action("NotCurrentlyAvailableDialogHandler", &["OK"]));
    }
    if text.contains("too many failed login attempts") {
        return Some(action("TooManyFailedLoginAttemptsDialogHandler", &["OK"]));
    }
    if title.contains("connection lost") || text.contains("connection lost") {
        return Some(action("ConnectionLostDialogHandler", &["OK", "Reconnect"]));
    }
    if title.contains("password notice") && policy.dismiss_password_expiry {
        return Some(action("PasswordExpiryWarningFrameHandler", &["OK"]));
    }
    if text.contains("bid, ask and last size display update")
        && policy.accept_bid_ask_last_size_update
    {
        return Some(action(
            "BidAskLastSizeDisplayUpdateDialogHandler",
            &["I understand - display market data", "OK"],
        ));
    }
    if title.contains("cryptocurrency order confirmation") {
        match policy.crypto_order_decision {
            CryptoOrderDecision::Transmit => {
                return Some(action(
                    "CryptoOrderConfirmationDialogHandler",
                    &["Transmit"],
                ));
            }
            CryptoOrderDecision::Cancel => {
                return Some(action("CryptoOrderConfirmationDialogHandler", &["Cancel"]));
            }
            CryptoOrderDecision::Manual => return None,
        }
    }
    if (title.contains("order preview") || text.contains("order preview"))
        && !text.contains("blind trading")
        && policy.bypass_order_precautions
    {
        return Some(action("OrderPreviewDialogHandler", &["Transmit", "Submit", "OK"]));
    }
    if (title.contains("precaution")
        || text.contains("precautionary setting")
        || text.contains("order precaution"))
        && policy.bypass_order_precautions
    {
        return Some(action(
            "ApiPrecautionWarningDialog",
            &["Override and Transmit", "Transmit", "Yes", "OK"],
        ));
    }
    if (title.contains("order preview") || text.contains("blind trading"))
        && text.contains("blind trading")
        && policy.allow_blind_trading
    {
        return Some(action(
            "BlindTradingWarningDialogHandler",
            &["Override and Transmit", "Yes"],
        ));
    }
    if title.contains("api client needs write access")
        || text.contains("api write access")
        || text.contains("read-only api")
    {
        return Some(action("ReadOnlyApiWarningDialog", &["Yes", "OK", "Close"]));
    }
    if text.contains("login handoff") || text.contains("trading login handoff") {
        return Some(action("TradingLoginHandoffDialogHandler", &["OK"]));
    }

    None
}

fn action(name: &'static str, buttons: &'static [&'static str]) -> DialogAction {
    DialogAction { name, buttons }
}

fn dialog_text_blob(window: &WindowInfo, components: &Value) -> String {
    let mut parts = vec![window.title.clone()];
    collect_strings(components, &mut parts);
    parts.join("\n")
}

fn collect_strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) if !s.trim().is_empty() => out.push(s.clone()),
        Value::Array(items) => {
            for item in items {
                collect_strings(item, out);
            }
        }
        Value::Object(map) => {
            for value in map.values() {
                collect_strings(value, out);
            }
        }
        _ => {}
    }
}

fn env_yes(name: &str) -> bool {
    env_lower(name)
        .as_deref()
        .is_some_and(|value| matches!(value, "yes" | "true" | "1"))
}

fn env_lower(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_inventory_tracks_27_plus_ibc_dialogs() {
        assert!(IBC_DIALOG_COVERAGE.len() >= 27);
        let unique = IBC_DIALOG_COVERAGE
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), IBC_DIALOG_COVERAGE.len());
    }

    #[test]
    fn classifies_api_change_confirmation_by_text() {
        let action = classify_dialog("Confirm API setting", "apply the new socket port setting")
            .expect("dialog should match");
        assert_eq!(action.name, "ApiChangeConfirmationDialogHandler");
        assert_eq!(action.buttons, &["Yes", "OK"]);
    }

    #[test]
    fn classifies_login_failures_by_title() {
        assert_eq!(
            classify_dialog("Login failed", "").unwrap().name,
            "LoginFailedDialogHandler"
        );
        assert_eq!(
            classify_dialog("Login Error", "").unwrap().name,
            "LoginErrorDialogHandler"
        );
    }

    #[test]
    fn classifies_not_currently_available_by_title_and_text() {
        let action = classify_dialog("Login", "The system is not currently available")
            .expect("dialog should match");
        assert_eq!(action.name, "NotCurrentlyAvailableDialogHandler");
    }

    #[test]
    fn risky_order_confirmations_default_to_manual() {
        let policy = DialogPolicy::default();
        assert!(
            classify_dialog_with_policy("Cryptocurrency order confirmation", "", &policy).is_none()
        );
        assert!(classify_dialog_with_policy("Order Preview", "blind trading", &policy).is_none());
    }

    #[test]
    fn explicit_order_policies_choose_safe_buttons() {
        let crypto = DialogPolicy {
            crypto_order_decision: CryptoOrderDecision::Cancel,
            ..Default::default()
        };
        assert_eq!(
            classify_dialog_with_policy("Cryptocurrency order confirmation", "", &crypto)
                .unwrap()
                .buttons,
            &["Cancel"]
        );

        let blind = DialogPolicy {
            allow_blind_trading: true,
            ..Default::default()
        };
        assert_eq!(
            classify_dialog_with_policy("Order Preview", "blind trading", &blind)
                .unwrap()
                .buttons,
            &["Override and Transmit", "Yes"]
        );
    }

    #[test]
    fn classifies_long_tail_ibc_dialogs() {
        let policy = DialogPolicy {
            dismiss_password_expiry: true,
            accept_bid_ask_last_size_update: true,
            bypass_order_precautions: true,
            ..Default::default()
        };

        let cases = [
            (
                "Restart Confirmation",
                "The platform will auto-restart rather than auto logoff",
                "AutoLogoffRestartConfirmationDialog",
            ),
            (
                "Warning",
                "connection lost to IB server",
                "ConnectionLostDialogHandler",
            ),
            ("Password Notice", "", "PasswordExpiryWarningFrameHandler"),
            (
                "Notice",
                "Bid, ask and last size display update",
                "BidAskLastSizeDisplayUpdateDialogHandler",
            ),
            (
                "Order Preview",
                "Please review this order preview",
                "OrderPreviewDialogHandler",
            ),
            (
                "Order Precaution",
                "This order triggers a precautionary setting",
                "ApiPrecautionWarningDialog",
            ),
            (
                "API client needs write access action confirmation",
                "",
                "ReadOnlyApiWarningDialog",
            ),
            (
                "Trading Login Handoff",
                "Trading login handoff is required",
                "TradingLoginHandoffDialogHandler",
            ),
        ];

        for (title, text, expected) in cases {
            assert_eq!(
                classify_dialog_with_policy(title, text, &policy).unwrap().name,
                expected
            );
        }
    }

    #[test]
    fn blind_trading_stays_manual_even_when_precautions_are_bypassed() {
        let policy = DialogPolicy {
            bypass_order_precautions: true,
            ..Default::default()
        };

        assert!(
            classify_dialog_with_policy("Order Preview", "blind trading", &policy).is_none()
        );
    }
}
