//! TOTP (Time-based One-Time Password) generation for two-factor authentication.
//!
//! The default-compatible implementation shells out to `oathtool`, piping the
//! secret via stdin to avoid exposing it in /proc/PID/cmdline. A built-in
//! RFC 6238 implementation is also available for images that do not want to
//! depend on an external TOTP executable.

use crate::config::TotpProvider;
use crate::types::TotpCode;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TotpError {
    #[error("failed to execute oathtool: {0}")]
    ExecutionFailed(#[from] std::io::Error),
    #[error("oathtool returned non-zero exit code: {0}")]
    OathtoolFailed(String),
    #[error("invalid base32 TOTP secret: {0}")]
    InvalidSecret(String),
    #[error("system clock is before the Unix epoch")]
    ClockBeforeUnixEpoch,
}

/// Trait for TOTP code generation providers.
pub trait TotpCodeGenerator: Send + Sync {
    /// Generate a 6-digit TOTP code from a base32-encoded secret.
    fn generate(&self, secret: &str) -> Result<TotpCode, TotpError>;

    /// Generate for an explicitly verified Unix timestamp.
    fn generate_at(&self, secret: &str, timestamp: u64) -> Result<TotpCode, TotpError>;
}

/// TOTP provider that shells out to the `oathtool` command-line utility.
///
/// The secret is piped via stdin (not passed as a command-line argument)
/// to prevent exposure in /proc/PID/cmdline.
pub struct OathtoolProvider;

impl TotpCodeGenerator for OathtoolProvider {
    fn generate(&self, secret: &str) -> Result<TotpCode, TotpError> {
        let timestamp = crate::time_sync::corrected_unix_seconds()
            .map_err(|_| TotpError::ClockBeforeUnixEpoch)?;
        self.generate_at(secret, timestamp)
    }

    fn generate_at(&self, secret: &str, timestamp: u64) -> Result<TotpCode, TotpError> {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let mut child = Command::new("oathtool")
            .args(["--totp", "--base32", &format!("--now=@{timestamp}"), "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        // Write secret to stdin and close it
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(secret.as_bytes())?;
        }

        let output = child.wait_with_output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(TotpError::OathtoolFailed(stderr.to_string()));
        }

        let code = String::from_utf8_lossy(&output.stdout).trim().to_string();
        log::debug!("Generated TOTP code (length={})", code.len());
        Ok(TotpCode::new(code))
    }
}

/// Built-in RFC 6238 TOTP provider using HMAC-SHA1, 30-second steps, and
/// 6-digit codes. This matches `oathtool --totp --base32`.
pub struct BuiltinProvider;

impl TotpCodeGenerator for BuiltinProvider {
    fn generate(&self, secret: &str) -> Result<TotpCode, TotpError> {
        let timestamp = crate::time_sync::corrected_unix_seconds()
            .map_err(|_| TotpError::ClockBeforeUnixEpoch)?;

        self.generate_at(secret, timestamp)
    }

    fn generate_at(&self, secret: &str, timestamp: u64) -> Result<TotpCode, TotpError> {
        let code = generate_totp_at(secret, timestamp, 30, 6)?;
        log::debug!("Generated built-in TOTP code (length={})", code.len());
        Ok(TotpCode::new(code))
    }
}

/// Wait out the dangerous edge of a 30-second TOTP window. Codes generated
/// in the final two seconds are likely to expire while the UI is typing them.
pub async fn wait_for_safe_window() {
    let Ok(now) = crate::time_sync::corrected_unix_seconds() else { return; };
    let position = now % 30;
    let wait = if position >= 27 { 32 - position } else if position < 2 { 2 - position } else { 0 };
    if wait > 0 {
        log::info!("TOTP boundary guard: waiting {}s before code generation", wait);
        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
    }
}

/// Factory function to create a TOTP code generator by provider type.
pub fn create_provider(provider: TotpProvider) -> Result<Box<dyn TotpCodeGenerator>, TotpError> {
    match provider {
        TotpProvider::Oathtool => Ok(Box::new(OathtoolProvider)),
        TotpProvider::Builtin => Ok(Box::new(BuiltinProvider)),
    }
}

fn generate_totp_at(
    secret: &str,
    timestamp: u64,
    step_seconds: u64,
    digits: u32,
) -> Result<String, TotpError> {
    if step_seconds == 0 {
        return Err(TotpError::InvalidSecret(
            "TOTP time step must be greater than zero".to_string(),
        ));
    }
    if !(6..=8).contains(&digits) {
        return Err(TotpError::InvalidSecret(
            "TOTP digit count must be between 6 and 8".to_string(),
        ));
    }

    let key = decode_base32(secret)?;
    let counter = timestamp / step_seconds;
    let counter_bytes = counter.to_be_bytes();
    let digest = hmac_sha1(&key, &counter_bytes);
    let offset = (digest[19] & 0x0f) as usize;
    let binary = ((u32::from(digest[offset]) & 0x7f) << 24)
        | (u32::from(digest[offset + 1]) << 16)
        | (u32::from(digest[offset + 2]) << 8)
        | u32::from(digest[offset + 3]);
    let modulo = 10_u32.pow(digits);

    Ok(format!(
        "{:0width$}",
        binary % modulo,
        width = digits as usize
    ))
}

fn decode_base32(secret: &str) -> Result<Vec<u8>, TotpError> {
    let mut bits: u32 = 0;
    let mut bit_count: u8 = 0;
    let mut out = Vec::new();
    let mut saw_padding = false;

    for ch in secret.chars() {
        if ch.is_ascii_whitespace() || ch == '-' {
            continue;
        }
        if ch == '=' {
            saw_padding = true;
            continue;
        }
        if saw_padding {
            return Err(TotpError::InvalidSecret(
                "non-padding data found after base32 padding".to_string(),
            ));
        }

        let value = match ch.to_ascii_uppercase() {
            'A'..='Z' => ch.to_ascii_uppercase() as u8 - b'A',
            '2'..='7' => ch as u8 - b'2' + 26,
            _ => {
                return Err(TotpError::InvalidSecret(format!(
                    "unexpected character '{}'",
                    ch
                )))
            }
        };

        bits = (bits << 5) | u32::from(value);
        bit_count += 5;

        while bit_count >= 8 {
            let shift = bit_count - 8;
            out.push(((bits >> shift) & 0xff) as u8);
            bit_count -= 8;
            bits &= (1 << bit_count) - 1;
        }
    }

    if out.is_empty() {
        return Err(TotpError::InvalidSecret("secret is empty".to_string()));
    }

    Ok(out)
}

fn hmac_sha1(key: &[u8], message: &[u8]) -> [u8; 20] {
    const BLOCK_SIZE: usize = 64;

    let mut key_block = [0_u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        key_block[..20].copy_from_slice(&sha1(key));
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }

    let mut outer_pad = [0x5c_u8; BLOCK_SIZE];
    let mut inner_pad = [0x36_u8; BLOCK_SIZE];

    for i in 0..BLOCK_SIZE {
        outer_pad[i] ^= key_block[i];
        inner_pad[i] ^= key_block[i];
    }

    let mut inner = Vec::with_capacity(BLOCK_SIZE + message.len());
    inner.extend_from_slice(&inner_pad);
    inner.extend_from_slice(message);
    let inner_hash = sha1(&inner);

    let mut outer = Vec::with_capacity(BLOCK_SIZE + inner_hash.len());
    outer.extend_from_slice(&outer_pad);
    outer.extend_from_slice(&inner_hash);
    sha1(&outer)
}

fn sha1(input: &[u8]) -> [u8; 20] {
    let mut h0: u32 = 0x6745_2301;
    let mut h1: u32 = 0xefcd_ab89;
    let mut h2: u32 = 0x98ba_dcfe;
    let mut h3: u32 = 0x1032_5476;
    let mut h4: u32 = 0xc3d2_e1f0;

    let bit_len = (input.len() as u64) * 8;
    let mut msg = input.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut w = [0_u32; 80];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            let j = i * 4;
            *word = u32::from_be_bytes([chunk[j], chunk[j + 1], chunk[j + 2], chunk[j + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let mut a = h0;
        let mut b = h1;
        let mut c = h2;
        let mut d = h3;
        let mut e = h4;

        for (i, word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5a82_7999),
                20..=39 => (b ^ c ^ d, 0x6ed9_eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
                _ => (b ^ c ^ d, 0xca62_c1d6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut out = [0_u8; 20];
    out[..4].copy_from_slice(&h0.to_be_bytes());
    out[4..8].copy_from_slice(&h1.to_be_bytes());
    out[8..12].copy_from_slice(&h2.to_be_bytes());
    out[12..16].copy_from_slice(&h3.to_be_bytes());
    out[16..20].copy_from_slice(&h4.to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_totp_matches_rfc_6238_sha1_vectors() {
        let secret = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
        let vectors = [
            (59, "94287082"),
            (1_111_111_109, "07081804"),
            (1_111_111_111, "14050471"),
            (1_234_567_890, "89005924"),
            (2_000_000_000, "69279037"),
            (20_000_000_000, "65353130"),
        ];

        for (timestamp, expected) in vectors {
            assert_eq!(
                generate_totp_at(secret, timestamp, 30, 8).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn builtin_totp_returns_six_digits_by_default() {
        let secret = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
        assert_eq!(generate_totp_at(secret, 59, 30, 6).unwrap(), "287082");
    }

    #[test]
    fn base32_decoder_accepts_lowercase_spacing_and_padding() {
        assert_eq!(
            decode_base32("jbsw y3dp ehpk 3pxp====").unwrap(),
            b"Hello!\xde\xad\xbe\xef"
        );
    }

    #[test]
    fn base32_decoder_rejects_invalid_characters() {
        let err = decode_base32("not valid *").unwrap_err().to_string();
        assert!(err.contains("unexpected character '*'"));
    }

    #[test]
    fn create_builtin_provider_generates_code() {
        let provider = create_provider(TotpProvider::Builtin).unwrap();
        let code = provider
            .generate("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ")
            .unwrap()
            .into_inner();
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|ch| ch.is_ascii_digit()));
    }

    #[test]
    fn explicit_timestamp_is_deterministic() {
        let provider = BuiltinProvider;
        let first = provider.generate_at("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ", 59).unwrap().into_inner();
        let second = provider.generate_at("GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ", 59).unwrap().into_inner();
        assert_eq!(first, second);
    }
}
