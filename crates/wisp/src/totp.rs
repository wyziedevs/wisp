//! Two-factor codes (TOTP, RFC 6238): the six digits an authenticator app
//! shows, which change every 30 s.
//!
//! ```ignore
//! let secret = wisp::totp::secret();                    // keep it with the user
//! let uri = wisp::totp::uri("My App", &user.email, &secret); // as a QR code, or text
//! if !wisp::totp::check(&user.totp, &code) { return invalid("code", "Wrong code"); }
//! ```
//!
//! SHA-1, six digits, 30 s: what every authenticator app takes. A code
//! counts for the step before and after the present one too (clock drift).
//! A code works again until its step is over, so for the strictest check
//! keep the step [`check_step`] returns, and refuse one no later than it.
//! Six digits are guessable: put a `RateLimit` on the check, by user.

use crate::token::percent;
use crate::ws::Sha1;
use crate::{secure_eq, unix_now};
use std::fmt::Write;

const STEP: u64 = 30;
const DIGITS: usize = 6;

/// A new secret, 160 bits as base32, which is how apps take it.
pub fn secret() -> String {
    let mut out = String::with_capacity(32);
    let (mut bits, mut n) = (0u32, 0);
    for b in crate::sign::random::<20>() {
        bits = bits << 8 | b as u32;
        n += 8;
        while n >= 5 {
            n -= 5;
            out.push(ALPHABET[(bits >> n & 31) as usize] as char);
        }
    }
    out
}

const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Base32 (RFC 4648) to bytes, in either case, with the spaces apps show
/// and padding left out; `None` for anything else.
fn unbase32(s: &str) -> Option<Vec<u8>> {
    let (mut out, mut bits, mut n) = (Vec::new(), 0u32, 0);
    for c in s.bytes().filter(|&c| c != b' ' && c != b'=') {
        let v = ALPHABET.iter().position(|&a| a == c.to_ascii_uppercase())?;
        bits = bits << 5 | v as u32;
        n += 5;
        if n >= 8 {
            n -= 8;
            out.push((bits >> n) as u8);
        }
    }
    (!out.is_empty()).then_some(out)
}

/// The `otpauth://` address authenticator apps read from a QR code.
/// `issuer` names the app, `account` the user (an email).
pub fn uri(issuer: &str, account: &str, secret: &str) -> String {
    let mut out = String::from("otpauth://totp/");
    percent(&mut out, issuer);
    out.push(':');
    percent(&mut out, account);
    out.push_str("?secret=");
    percent(&mut out, secret);
    out.push_str("&issuer=");
    percent(&mut out, issuer);
    let _ = write!(out, "&algorithm=SHA1&digits={DIGITS}&period={STEP}");
    out
}

/// HMAC-SHA1 (RFC 2104) of `msg`.
fn hmac_sha1(key: &[u8], msg: &[u8]) -> [u8; 20] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        let mut h = Sha1::new();
        h.update(key);
        k[..20].copy_from_slice(&h.finish());
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha1::new();
    inner.update(&k.map(|b| b ^ 0x36));
    inner.update(msg);
    let mut outer = Sha1::new();
    outer.update(&k.map(|b| b ^ 0x5c));
    outer.update(&inner.finish());
    outer.finish()
}

/// The code of `key` for time step `step` (HOTP, RFC 4226).
fn hotp(key: &[u8], step: u64) -> String {
    let mac = hmac_sha1(key, &step.to_be_bytes());
    let at = (mac[19] & 15) as usize;
    let n = u32::from_be_bytes([mac[at] & 0x7f, mac[at + 1], mac[at + 2], mac[at + 3]]);
    format!("{:0DIGITS$}", n % 10u32.pow(DIGITS as u32))
}

/// The code `secret` gives at `time` (Unix seconds), for an app's own
/// tests; `None` for a secret that is not base32.
pub fn code(secret: &str, time: u64) -> Option<String> {
    Some(hotp(&unbase32(secret)?, time / STEP))
}

/// Whether `code` is the present code of `secret`, or the one before or
/// after.
pub fn check(secret: &str, code: &str) -> bool {
    check_step(secret, code).is_some()
}

/// [`check`], and the time step `code` was for: keep it, and refuse a code
/// whose step is not later, so a code seen once (over a shoulder, in a
/// proxy's log) cannot be used again.
pub fn check_step(secret: &str, code: &str) -> Option<u64> {
    check_at(secret, code, unix_now())
}

fn check_at(secret: &str, code: &str, time: u64) -> Option<u64> {
    let key = unbase32(secret)?;
    // The digits an app shows may be grouped ("123 456").
    let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    if code.len() != DIGITS || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // All three are compared whichever matches first, in time that does
    // not say which digit was wrong.
    let now = time / STEP;
    let mut found = None;
    for step in [now.saturating_sub(1), now, now + 1] {
        if secure_eq(hotp(&key, step), &code) {
            found = Some(step);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238 Appendix B: the key "12345678901234567890", SHA-1; the
    /// RFC shows 8 digits, whose last 6 are ours.
    #[test]
    fn rfc_6238_vectors() {
        let key = b"12345678901234567890";
        for (time, eight) in [
            (59u64, "94287082"),
            (1111111109, "07081804"),
            (1111111111, "14050471"),
            (1234567890, "89005924"),
            (2000000000, "69279037"),
            (20000000000, "65353130"),
        ] {
            assert_eq!(hotp(key, time / STEP), eight[2..], "at {time}");
        }
    }

    /// RFC 4226 Appendix D, the first counters.
    #[test]
    fn rfc_4226_vectors() {
        let key = b"12345678901234567890";
        let got: Vec<_> = (0..4).map(|c| hotp(key, c)).collect();
        assert_eq!(got, ["755224", "287082", "359152", "969429"]);
    }

    #[test]
    fn hmac_sha1_rfc_2202() {
        let hex = |b: [u8; 20]| crate::hex(&b);
        assert_eq!(
            hex(hmac_sha1(&[0x0b; 20], b"Hi There")),
            "b617318655057264e28bc0b6fb378c8ef146be00"
        );
        assert_eq!(
            hex(hmac_sha1(b"Jefe", b"what do ya want for nothing?")),
            "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79"
        );
        // A key longer than a block is hashed first.
        assert_eq!(
            hex(hmac_sha1(
                &[0xaa; 80],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )),
            "aa4ae5e15272d00e95705637ce8a3b55ed402112"
        );
    }

    #[test]
    fn secrets_and_uris() {
        let s = secret();
        assert_eq!(s.len(), 32);
        assert!(s.bytes().all(|b| ALPHABET.contains(&b)));
        assert_ne!(s, secret());
        assert_eq!(unbase32(&s).unwrap().len(), 20);
        assert_eq!(unbase32("mfrg gzdf").unwrap(), b"abcde");
        assert!(unbase32("").is_none() && unbase32("a1").is_none());
        assert_eq!(
            uri("My App", "a+b@x.com", "JBSWY3DP"),
            "otpauth://totp/My%20App:a%2Bb%40x.com?secret=JBSWY3DP&issuer=My%20App&algorithm=SHA1&digits=6&period=30"
        );
    }

    #[test]
    fn checks_a_window_and_nothing_else() {
        let s = secret();
        let t = 1_700_000_000;
        let at = |time| code(&s, time).unwrap();
        assert_eq!(check_at(&s, &at(t), t), Some(t / STEP));
        assert_eq!(check_at(&s, &at(t - STEP), t), Some(t / STEP - 1), "drift");
        assert_eq!(check_at(&s, &at(t + STEP), t), Some(t / STEP + 1));
        assert!(check_at(&s, &at(t - 2 * STEP), t).is_none());
        assert!(check_at(&s, &at(t + 2 * STEP), t).is_none());
        let grouped = format!("{} {}", &at(t)[..3], &at(t)[3..]);
        assert!(check_at(&s, &grouped, t).is_some());
        // A wrong code is None, and never a panic, whatever it holds.
        let right = at(t);
        let wrong = format!("{:06}", (right.parse::<u32>().unwrap() + 1) % 1_000_000);
        for bad in [
            "",
            "12345",
            "1234567",
            "12345a",
            "٣٣٣٣٣٣",
            "+12345",
            "-12345",
            &wrong,
        ] {
            assert!(check_at(&s, bad, t).is_none(), "{bad:?}");
        }
        assert!(check_at("not base32!", &right, t).is_none());
        assert!(check_at("", &right, t).is_none());
        assert!(check(&s, &code(&s, unix_now()).unwrap()));
    }
}
