//! Accounts (1.25.0+): the pieces that do not need the server state. Authenticator app
//! codes (TOTP, RFC 6238: SHA-1, 6 digits, 30 seconds, like Google Authenticator and
//! Yubico Authenticator), recovery codes, remembered-device tokens and the temporary
//! passwords of an administrator's reset.

use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::{Digest, Sha256};

const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Base32 (RFC 4648) without padding, as authenticator apps expect.
pub fn base32(data: &[u8]) -> String {
    let mut out = String::new();
    let (mut buf, mut bits) = (0u32, 0u32);
    for &b in data {
        buf = (buf << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            out.push(B32[((buf >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
    }
    if bits > 0 {
        out.push(B32[((buf << (5 - bits)) & 31) as usize] as char);
    }
    out
}

pub fn sha256_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Constant-time comparison of two hex digests.
pub fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn new_totp_secret() -> Vec<u8> {
    let mut s = vec![0u8; 20];
    rand::rngs::OsRng.fill_bytes(&mut s);
    s
}

fn totp_at(secret: &[u8], step: u64) -> u32 {
    let Ok(mut m) = Hmac::<sha1::Sha1>::new_from_slice(secret) else { return u32::MAX };
    m.update(&step.to_be_bytes());
    let h = m.finalize().into_bytes();
    let o = (h[19] & 0xf) as usize;
    let bin = ((h[o] as u32 & 0x7f) << 24) | ((h[o + 1] as u32) << 16) | ((h[o + 2] as u32) << 8) | h[o + 3] as u32;
    bin % 1_000_000
}

/// Checks a 6-digit code (a step either side for clock drift). Returns the step it was
/// for; a step at or before `last_step` (a code already used) is refused.
pub fn check_totp(secret: &[u8], code: &str, now: u64, last_step: u64) -> Option<u64> {
    let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    if code.len() != 6 || !code.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let want: u32 = code.parse().ok()?;
    let now_step = now / 30;
    [now_step.saturating_sub(1), now_step, now_step + 1].into_iter().find(|&step| step > last_step && totp_at(secret, step) == want)
}

/// The link an authenticator app reads from the QR code.
pub fn otpauth_uri(issuer: &str, username: &str, secret: &[u8]) -> String {
    let enc = |s: &str| s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect::<String>();
    format!("otpauth://totp/{}:{}?secret={}&issuer={}&algorithm=SHA1&digits=6&period=30", enc(issuer), enc(username), base32(secret), enc(issuer))
}

/// Ten one-time recovery codes like `K7Q2M-XW4PA` (50 random bits each), and their
/// digests for storing.
pub fn new_recovery_codes() -> (Vec<String>, Vec<String>) {
    let mut codes = Vec::new();
    for _ in 0..10 {
        let mut b = [0u8; 8];
        rand::rngs::OsRng.fill_bytes(&mut b);
        let s = base32(&b);
        codes.push(format!("{}-{}", &s[..5], &s[5..10]));
    }
    let hashes = codes.iter().map(|c| sha256_hex(&normalize_code(c))).collect();
    (codes, hashes)
}

/// Recovery and reset codes are compared without dashes, spaces or case.
pub fn normalize_code(c: &str) -> String {
    c.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect()
}

/// A random token (hex): remembered devices.
pub fn new_token() -> String {
    let mut b = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A temporary password for an administrator's reset, like `K7Q2M-XW4PA-M3D8Q` (75
/// random bits; letters and digits that are easy to read out).
pub fn new_temp_password() -> String {
    let mut b = [0u8; 10];
    rand::rngs::OsRng.fill_bytes(&mut b);
    let s = base32(&b);
    format!("{}-{}-{}", &s[..5], &s[5..10], &s[10..15])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totp_rfc6238_vectors() {
        // RFC 6238 appendix B (SHA-1, 8 digits there; the last 6 digits here).
        let secret = b"12345678901234567890";
        assert_eq!(totp_at(secret, 59 / 30), 94287082 % 1_000_000);
        assert_eq!(totp_at(secret, 1111111109 / 30), 7081804 % 1_000_000);
        assert_eq!(totp_at(secret, 1234567890 / 30), 89005924 % 1_000_000);
        let code = format!("{:06}", totp_at(secret, 1234567890 / 30));
        assert_eq!(check_totp(secret, &code, 1234567890, 0), Some(1234567890 / 30));
        // A code works once.
        assert_eq!(check_totp(secret, &code, 1234567890, 1234567890 / 30), None);
    }

    #[test]
    fn base32_rfc4648() {
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
        assert_eq!(base32(b"f"), "MY");
    }

    #[test]
    fn temp_passwords() {
        let p = new_temp_password();
        assert_eq!(p.len(), 17);
        assert!(voxa_proto::valid_account_password(&p));
    }

    #[test]
    fn recovery_codes() {
        let (codes, hashes) = new_recovery_codes();
        assert_eq!(codes.len(), 10);
        assert_eq!(sha256_hex(&normalize_code(&codes[0].to_lowercase())), hashes[0]);
    }
}
