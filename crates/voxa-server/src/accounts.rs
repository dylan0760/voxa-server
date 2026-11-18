//! Accounts (1.25.0+): the pieces that do not need the server state. Authenticator app
//! codes (TOTP, RFC 6238: SHA-1, 6 digits, 30 seconds, like Google Authenticator and
//! Yubico Authenticator), recovery codes, remembered-device tokens and reset emails.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::config::Config;

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

/// An 8-digit code for a reset email.
pub fn new_reset_code() -> String {
    format!("{:08}", rand::rngs::OsRng.next_u64() % 100_000_000)
}

/// `d***@example.com`
pub fn mask_email(e: &str) -> String {
    match e.split_once('@') {
        Some((user, domain)) => format!("{}***@{domain}", user.chars().next().unwrap_or('*')),
        None => "***".into(),
    }
}

/// A plausible email address (one @, a dot after it, no spaces or line breaks).
pub fn valid_email(e: &str) -> bool {
    let e = e.trim();
    e.len() <= 254
        && !e.chars().any(|c| c.is_whitespace() || c.is_control() || c == '<' || c == '>' || c == ',')
        && e.split_once('@').is_some_and(|(u, d)| !u.is_empty() && d.contains('.') && !d.starts_with('.') && !d.ends_with('.') && !d.contains('@'))
}

pub fn smtp_configured(cfg: &Config) -> bool {
    !cfg.smtp_host.trim().is_empty() && !cfg.smtp_from.trim().is_empty()
}

/// Sends a plain-text email over SMTP with TLS (STARTTLS or implicit TLS; never in the
/// clear). Blocking: run it on a blocking thread.
pub fn send_mail(cfg: &Config, to: &str, subject: &str, body: &str) -> Result<(), String> {
    if !valid_email(to) {
        return Err("bad address".into());
    }
    let host = cfg.smtp_host.trim();
    let tcp = TcpStream::connect((host, cfg.smtp_port)).map_err(|e| format!("connect: {e}"))?;
    tcp.set_read_timeout(Some(Duration::from_secs(30))).ok();
    tcp.set_write_timeout(Some(Duration::from_secs(30))).ok();
    let tls = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
    let implicit = cfg.smtp_security.eq_ignore_ascii_case("tls");
    let mut s: Box<dyn RW> = if implicit {
        Box::new(tls.connect(host, tcp).map_err(|e| format!("TLS: {e}"))?)
    } else {
        let mut plain = BufReader::new(tcp);
        expect(&mut plain, 220)?;
        let ehlo = "EHLO voxa-server\r\n";
        plain.get_mut().write_all(ehlo.as_bytes()).map_err(|e| e.to_string())?;
        expect(&mut plain, 250)?;
        plain.get_mut().write_all(b"STARTTLS\r\n").map_err(|e| e.to_string())?;
        expect(&mut plain, 220)?;
        Box::new(tls.connect(host, plain.into_inner()).map_err(|e| format!("TLS: {e}"))?)
    };
    let mut r = BufReader::new(&mut s);
    if implicit {
        expect(&mut r, 220)?;
    }
    cmd(&mut r, "EHLO voxa-server", 250)?;
    if !cfg.smtp_user.is_empty() {
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD;
        let auth = b64.encode(format!("\0{}\0{}", cfg.smtp_user, cfg.smtp_password));
        cmd(&mut r, &format!("AUTH PLAIN {auth}"), 235)?;
    }
    let from_addr = cfg.smtp_from.rsplit_once('<').map(|(_, a)| a.trim_end_matches('>').trim()).unwrap_or(cfg.smtp_from.trim());
    cmd(&mut r, &format!("MAIL FROM:<{from_addr}>"), 250)?;
    cmd(&mut r, &format!("RCPT TO:<{}>", to.trim()), 250)?;
    cmd(&mut r, "DATA", 354)?;
    // No user text in headers except the checked address; the body has no lone dots.
    let clean = |t: &str| t.replace(['\r', '\n'], " ");
    let body = body.replace("\r\n", "\n").lines().map(|l| if l.starts_with('.') { format!(".{l}") } else { l.to_string() }).collect::<Vec<_>>().join("\r\n");
    let msg = format!(
        "From: {}\r\nTo: <{}>\r\nSubject: {}\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: 8bit\r\n\r\n{}\r\n.",
        clean(&cfg.smtp_from),
        to.trim(),
        clean(subject),
        body
    );
    cmd(&mut r, &msg, 250)?;
    let _ = cmd(&mut r, "QUIT", 221);
    Ok(())
}

trait RW: Read + Write {}
impl<T: Read + Write> RW for T {}

fn expect(r: &mut impl BufRead, code: u16) -> Result<(), String> {
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return Err("the mail server closed the connection".into());
        }
        let got: u16 = line.get(..3).and_then(|c| c.parse().ok()).unwrap_or(0);
        // "250-..." continues, "250 ..." ends.
        if line.as_bytes().get(3) == Some(&b'-') {
            continue;
        }
        return if got == code { Ok(()) } else { Err(format!("the mail server said: {}", line.trim())) };
    }
}

fn cmd<S: Read + Write>(r: &mut BufReader<S>, line: &str, code: u16) -> Result<(), String> {
    r.get_mut().write_all(format!("{line}\r\n").as_bytes()).map_err(|e| e.to_string())?;
    r.get_mut().flush().map_err(|e| e.to_string())?;
    expect(r, code)
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
    fn emails() {
        assert!(valid_email("dylan@example.com"));
        assert!(!valid_email("a@b"));
        assert!(!valid_email("a@b.com\r\nBcc: x@y.z"));
        assert!(!valid_email("<a@b.com>"));
        assert_eq!(mask_email("dylan@example.com"), "d***@example.com");
    }

    #[test]
    fn recovery_codes() {
        let (codes, hashes) = new_recovery_codes();
        assert_eq!(codes.len(), 10);
        assert_eq!(sha256_hex(&normalize_code(&codes[0].to_lowercase())), hashes[0]);
    }
}
