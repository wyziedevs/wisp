//! Signed expiring tokens: a password reset, an email check, a magic link.
//!
//! ```ignore
//! let link = format!("/reset?t={}", wisp::token("reset", &user.id, Duration::from_secs(3600)));
//! let id: u64 = wisp::untoken("reset", &t)?;   // 400 unless it is ours, this purpose, and fresh
//! ```
//!
//! A token is `payload.expiry.signature`: HMAC-SHA256 under `WISP_SECRET`
//! (`WISP_SECRET_OLD` still checks, as for cookies) over the purpose, the
//! payload and the expiry, so none of the three can be changed, and a
//! token made for one purpose is nobody's for another. Nothing is
//! encrypted: the payload is readable, so put an id in it, not a secret.
//! A token is not used up: it works until it expires. For a reset, put in
//! it something that changes once it is used (a hash's first bytes), and
//! check that. Every failure is the same 400, so which part was wrong is
//! not told.

use crate::json::{FromJson, from_json, to_json};
use crate::live::Json;
use crate::sign::{base64, cookie_mac, unbase64, verify_cookie};
use crate::{Error, Result, unix_now};
use std::time::Duration;

/// Cookie names cannot hold a space, so a signed cookie's signature is
/// never a token's, nor the reverse.
const NAME: &str = "wisp token";

/// Longest token read: a forged one costs no more than this to refuse.
const MAX: usize = 4096;

/// A token holding `value`, good for `ttl`, for one `purpose` (`"reset"`,
/// `"verify"`, or `""` for none): [`untoken`] with another purpose refuses
/// it. Panics on `.` or a newline in `purpose`.
pub fn token(purpose: &str, value: &(impl Json + ?Sized), ttl: Duration) -> String {
    assert!(
        !purpose.contains(['.', '\n']),
        "a token's purpose is a word, not {purpose:?}"
    );
    let mut out = String::new();
    base64(&mut out, to_json(value).as_bytes(), true);
    let expiry = unix_now().saturating_add(ttl.as_secs());
    out.push('.');
    out.push_str(&expiry.to_string());
    let mac = cookie_mac(NAME, &format!("{purpose}\n{out}"));
    out.push('.');
    base64(&mut out, &mac, true);
    out
}

/// The value in `token`, if Wisp made it for `purpose`, which [`token`] was
/// given, and it has not expired; else a 400.
pub fn untoken<T: FromJson>(purpose: &str, token: &str) -> Result<T> {
    read(purpose, token).ok_or_else(|| {
        Error::new(400, "This link is not valid, or has expired").with_code("bad_token")
    })
}

fn read<T: FromJson>(purpose: &str, token: &str) -> Option<T> {
    if token.len() > MAX || purpose.contains(['.', '\n']) {
        return None;
    }
    let mut parts = token.split('.');
    let (payload, expiry, sig) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let signed = format!("{purpose}\n{payload}.{expiry}");
    // Nothing is parsed before the signature holds.
    if !verify_cookie(NAME, &signed, sig) || expiry.parse::<u64>().ok()? < unix_now() {
        return None;
    }
    let mut bytes = vec![0; payload.len() / 4 * 3 + 3];
    let n = unbase64(payload, &mut bytes)?;
    from_json(&bytes[..n]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: Duration = Duration::from_secs(3600);

    fn base64_of(s: &str) -> String {
        let mut out = String::new();
        base64(&mut out, s.as_bytes(), true);
        out
    }

    /// `body.expiry.signature` signed under `name` and `purpose`.
    fn made(name: &str, purpose: &str, body: &str, expiry: u64) -> String {
        let mut t = format!("{}.{expiry}", base64_of(body));
        let mac = cookie_mac(name, &format!("{purpose}\n{t}"));
        t.push('.');
        base64(&mut t, &mac, true);
        t
    }

    #[test]
    fn round_trip() {
        let t = token("", &42u64, HOUR);
        assert_eq!(untoken::<u64>("", &t).unwrap(), 42);
        let s = token("reset", "ada@example.com", HOUR);
        assert_eq!(untoken::<String>("reset", &s).unwrap(), "ada@example.com");
        assert!(!s.contains(['+', '/', '=']), "safe in a URL");
    }

    #[test]
    fn expired_forged_and_wrong_purpose_are_all_the_same_400() {
        let fresh = made(NAME, "", "1", unix_now());
        assert_eq!(
            untoken::<u64>("", &fresh).unwrap(),
            1,
            "this second still counts"
        );
        let gone = made(NAME, "", "1", unix_now() - 1);
        let e = untoken::<u64>("", &gone).unwrap_err();
        assert_eq!((e.status(), e.code()), (400, "bad_token"));

        let reset = token("reset", &7u64, HOUR);
        let wrong = untoken::<u64>("verify", &reset).unwrap_err();
        assert_eq!(wrong.message(), e.message());
        assert!(
            untoken::<u64>("", &reset).is_err(),
            "no purpose is a purpose"
        );

        // Another payload or expiry under the same signature.
        let other = token("reset", &8u64, HOUR);
        let a: Vec<_> = reset.split('.').collect();
        let b: Vec<_> = other.split('.').collect();
        let spliced = format!("{}.{}.{}", b[0], a[1], a[2]);
        assert!(untoken::<u64>("reset", &spliced).is_err());
        let longer = format!("{}.{}.{}", a[0], unix_now() + 999_999, a[2]);
        assert!(untoken::<u64>("reset", &longer).is_err());
    }

    /// A signed cookie's signature is not a token's.
    #[test]
    fn a_signed_cookie_is_not_a_token() {
        let forged = made("token", "", "5", unix_now() + 999);
        assert!(untoken::<u64>("", &forged).is_err());
        assert_eq!(
            untoken::<u64>("", &made(NAME, "", "5", unix_now() + 999)).unwrap(),
            5
        );
    }

    #[test]
    fn mangled_tokens_are_refused_and_never_panic() {
        use crate::fuzz::{Rng, mutate};
        let good = token("x", &99u64, HOUR);
        let mut rng = Rng::new(9);
        for _ in 0..20_000 {
            let mut b = good.clone().into_bytes();
            mutate(&mut rng, &mut b);
            if let Ok(s) = String::from_utf8(b) {
                let got = untoken::<u64>("x", &s).ok();
                assert!(got.is_none() || got == Some(99), "{s:?}");
            }
        }
        for s in ["", ".", "..", "...", "a.b.c", &"A".repeat(MAX + 1)] {
            assert!(untoken::<u64>("", s).is_err());
        }
    }
}
