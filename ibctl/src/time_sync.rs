//! Authoritative time verification for TOTP generation.
//!
//! Containers cannot safely set the Docker Desktop host clock.  Instead we
//! sample several NTP servers, keep a bounded median offset, and apply that
//! offset only to TOTP generation.  A best-effort request marker is also
//! written for the optional Windows host task.

use std::net::{ToSocketAddrs, UdpSocket};
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const NTP_UNIX_DELTA_SECS: f64 = 2_208_988_800.0;
const MAX_ACCEPTED_OFFSET_MS: i64 = 120_000;
const DEFAULT_SERVERS: &[&str] = &[
    "time.cloudflare.com:123",
    "time.google.com:123",
    "pool.ntp.org:123",
];

static VERIFIED_OFFSET_MS: AtomicI64 = AtomicI64::new(0);
static LAST_VERIFIED_UNIX_SECS: AtomicI64 = AtomicI64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeSyncResult {
    pub offset_ms: i64,
    pub samples: usize,
}

pub fn corrected_unix_seconds() -> Result<u64, std::time::SystemTimeError> {
    let local_ms = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as i128;
    let corrected = local_ms + i128::from(VERIFIED_OFFSET_MS.load(Ordering::Relaxed));
    Ok(corrected.max(0) as u64 / 1000)
}

pub fn verified_offset_ms() -> i64 {
    VERIFIED_OFFSET_MS.load(Ordering::Relaxed)
}

pub fn last_verified_unix_secs() -> i64 {
    LAST_VERIFIED_UNIX_SECS.load(Ordering::Relaxed)
}

pub fn is_twofa_failure(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let auth_context = [
        "security code",
        "authentication code",
        "verification code",
        "2fa",
        "two-factor",
        "second factor",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    let failure = [
        "incorrect",
        "invalid",
        "failed",
        "not valid",
        "try again",
        "retry",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    auth_context && failure
}

/// Extract Gateway's server-enforced login delay from text such as
/// "Please try again in 43 seconds" or "Retry after 2 minutes".
pub fn parse_retry_seconds(text: &str) -> Option<u64> {
    let lower = text.to_ascii_lowercase();
    // Parse only the sentence containing retry language. This prevents an
    // unrelated duration elsewhere in a multi-label dialog from being used.
    for segment in lower.split(['.', '!', '\n', ';']) {
        let retry_context = ["retry", "try again", "wait", "attempt again", "until retry"]
            .iter()
            .any(|needle| segment.contains(needle));
        if !retry_context {
            continue;
        }

        // Some Gateway builds render the countdown as mm:ss.
        for token in segment.split_whitespace() {
            let trimmed = token.trim_matches(|ch: char| !ch.is_ascii_digit() && ch != ':');
            if let Some((minutes, seconds)) = trimmed.split_once(':') {
                if let (Ok(minutes), Ok(seconds)) = (minutes.parse::<u64>(), seconds.parse::<u64>()) {
                    if seconds < 60 {
                        return Some(minutes.saturating_mul(60).saturating_add(seconds).min(3600));
                    }
                }
            }
        }

        let normalized = segment.replace(|ch: char| !ch.is_ascii_alphanumeric(), " ");
        let words = normalized.split_whitespace().collect::<Vec<_>>();
        let mut total = 0_u64;
        let mut found = false;
        for (index, word) in words.iter().enumerate() {
            let Ok(value) = word.parse::<u64>() else {
                continue;
            };
            let unit = words.get(index + 1).copied().unwrap_or("");
            if unit.starts_with("sec") {
                total = total.saturating_add(value);
                found = true;
            }
            if unit.starts_with("min") {
                total = total.saturating_add(value.saturating_mul(60));
                found = true;
            }
        }
        if found {
            return Some(total.min(3600));
        }
    }
    None
}

pub async fn verify_now(reason: &'static str) -> Option<TimeSyncResult> {
    request_host_sync(reason);
    let servers = std::env::var("IBCTL_NTP_SERVERS")
        .ok()
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .filter(|items| !items.is_empty())
        .unwrap_or_else(|| DEFAULT_SERVERS.iter().map(|s| (*s).to_string()).collect());

    let result = tokio::task::spawn_blocking(move || verify_servers(&servers)).await;
    match result {
        Ok(Some(result)) => {
            VERIFIED_OFFSET_MS.store(result.offset_ms, Ordering::Relaxed);
            LAST_VERIFIED_UNIX_SECS.store(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64,
                Ordering::Relaxed,
            );
            log::info!(
                "time_sync.verified reason={} offset_ms={} samples={}",
                reason,
                result.offset_ms,
                result.samples,
            );
            Some(result)
        }
        Ok(None) => {
            log::warn!(
                "time_sync.unverified reason={} — retaining offset_ms={}",
                reason,
                verified_offset_ms()
            );
            None
        }
        Err(error) => {
            log::warn!("time_sync.task_failed reason={} error={}", reason, error);
            None
        }
    }
}

pub fn start_periodic_verifier(interval: Duration) {
    tokio::spawn(async move {
        let _ = verify_now("startup").await;
        loop {
            tokio::time::sleep(interval).await;
            let _ = verify_now("periodic").await;
        }
    });
}

fn verify_servers(servers: &[String]) -> Option<TimeSyncResult> {
    let mut offsets = servers
        .iter()
        .filter_map(|server| sample_ntp(server, Duration::from_secs(2)).ok())
        .filter(|offset| offset.abs() <= MAX_ACCEPTED_OFFSET_MS)
        .collect::<Vec<_>>();
    if offsets.is_empty() {
        return None;
    }
    offsets.sort_unstable();
    let offset_ms = offsets[offsets.len() / 2];
    Some(TimeSyncResult {
        offset_ms,
        samples: offsets.len(),
    })
}

fn sample_ntp(server: &str, timeout: Duration) -> Result<i64, String> {
    let addr = server
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or_else(|| format!("no address for {server}"))?;
    let socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    socket
        .set_read_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    socket
        .set_write_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    let mut packet = [0_u8; 48];
    packet[0] = 0x23; // client, NTPv4
    let t1 = system_time_secs();
    socket.send_to(&packet, addr).map_err(|e| e.to_string())?;
    let (size, _) = socket.recv_from(&mut packet).map_err(|e| e.to_string())?;
    let t4 = system_time_secs();
    if size < 48 || packet[1] == 0 {
        return Err("invalid NTP response".to_string());
    }
    let t2 = ntp_timestamp(&packet[32..40]);
    let t3 = ntp_timestamp(&packet[40..48]);
    let offset = ((t2 - t1) + (t3 - t4)) / 2.0;
    Ok((offset * 1000.0).round() as i64)
}

fn system_time_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn ntp_timestamp(bytes: &[u8]) -> f64 {
    let seconds = u32::from_be_bytes(bytes[0..4].try_into().unwrap()) as f64;
    let fraction = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as f64 / 4_294_967_296.0;
    seconds - NTP_UNIX_DELTA_SECS + fraction
}

fn request_host_sync(reason: &str) {
    let path = std::env::var("IBCTL_TIME_SYNC_REQUEST_FILE")
        .unwrap_or_else(|_| "/opt/ibctl/persist/time-sync/request".to_string());
    let Some(parent) = Path::new(&path).parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let body = format!("{now}\t{reason}\n");
    let temp = format!("{path}.tmp");
    if std::fs::write(&temp, body).is_ok() {
        let _ = std::fs::rename(temp, path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ntp_timestamp() {
        let unix = 1_700_000_000_u32;
        let ntp = unix as u64 + NTP_UNIX_DELTA_SECS as u64;
        let mut bytes = [0_u8; 8];
        bytes[..4].copy_from_slice(&(ntp as u32).to_be_bytes());
        assert!((ntp_timestamp(&bytes) - unix as f64).abs() < 0.001);
    }

    #[test]
    fn corrected_time_uses_verified_offset() {
        VERIFIED_OFFSET_MS.store(5_000, Ordering::Relaxed);
        let local = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let corrected = corrected_unix_seconds().unwrap();
        assert!((corrected as i64 - local as i64 - 5).abs() <= 1);
        VERIFIED_OFFSET_MS.store(0, Ordering::Relaxed);
    }

    #[test]
    fn parses_gateway_retry_countdown() {
        assert_eq!(
            parse_retry_seconds("Incorrect security code. Try again in 43 seconds."),
            Some(43)
        );
        assert_eq!(parse_retry_seconds("Retry after 2 minutes"), Some(120));
        assert_eq!(parse_retry_seconds("Try again in 1 minute 23 seconds"), Some(83));
        assert_eq!(parse_retry_seconds("Retry available in 01:17"), Some(77));
        assert_eq!(
            parse_retry_seconds("Try again in 43 seconds. Session expires in 5 minutes."),
            Some(43)
        );
        assert_eq!(parse_retry_seconds("Session duration 2 minutes"), None);
        assert_eq!(parse_retry_seconds("Login failed"), None);
    }

    #[test]
    fn classifies_only_auth_code_failures() {
        assert!(is_twofa_failure(
            "The security code is incorrect; retry in 30 seconds"
        ));
        assert!(!is_twofa_failure("Market data connection failed"));
    }
}
