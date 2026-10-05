//! Signed cookies: HMAC-SHA256 over the cookie's name and value, with the
//! app's secret.
//!
//! HMAC is written here and SHA-256 in `wisp_shared::sha256` rather than
//! pulled in, which keeps the runtime at two dependencies. Both are short,
//! fixed algorithms with published test vectors (FIPS 180-4, RFC 4231),
//! checked by their tests. Nothing is encrypted: a signed cookie's value is
//! readable, only not forgeable.

use crate::codec::hex_digit;
use std::cell::RefCell;
use std::path::Path;
use std::sync::OnceLock;
pub(crate) use wisp_shared::base64::{decode as unbase64, encode as base64};
use wisp_shared::sha256::{Sha256, compress_words, sha256};

/// The signature of cookie `name` holding `value`; `None` without a secret
/// to sign with (logged once, naming `WISP_SECRET`).
pub(crate) fn cookie_mac(name: &str, value: &str) -> Option<[u8; 32]> {
    Some(mac(key()?, name, value))
}

fn mac(k: &Hmac, name: &str, value: &str) -> [u8; 32] {
    k.sign(&[name.as_bytes(), b"=", value.as_bytes()])
}

/// Whether `mac` is the signature of cookie `name` holding `value`, in time
/// that does not depend on where they differ. One `WISP_SECRET_OLD` signed
/// holds too, so a new secret does not sign everyone out at once.
pub(crate) fn verify_cookie(name: &str, value: &str, sent: &str) -> bool {
    [key(), old_key()]
        .into_iter()
        .flatten()
        .any(|k| same_mac(sent, &mac(k, name, value)))
}

/// Whether `sent`, in hex or base64 (either alphabet, padded or not), is
/// `mac`, in time that does not depend on where they differ.
fn same_mac(sent: &str, mac: &[u8; 32]) -> bool {
    let mut got = [0u8; 32];
    let hex = sent.len() == 64
        && sent.as_bytes().chunks(2).zip(&mut got).all(|(p, g)| {
            match (hex_digit(p[0]), hex_digit(p[1])) {
                (Some(h), Some(l)) => {
                    *g = h << 4 | l;
                    true
                }
                _ => false,
            }
        });
    (hex || unbase64(sent, &mut got) == Some(32)) && crate::secure_eq(got, mac)
}

/// The project directory, for the dev secret. Set when the server starts.
pub(crate) static ROOT: OnceLock<&'static str> = OnceLock::new();

/// `WISP_SECRET`; in dev without it, a secret kept in the project's
/// `.wisp/secret`, so signed cookies survive restarts. Otherwise there is
/// none to sign with: `None`, logged once; a request that signs answers 500
/// and nothing signed verifies. Decided once, never a panic.
fn key() -> Option<&'static Hmac> {
    #[cfg(test)]
    if NO_KEY.get() {
        return None;
    }
    static KEY: OnceLock<Option<Hmac>> = OnceLock::new();
    KEY.get_or_init(|| {
        if let Some(secret) = &crate::settings().secret {
            return Some(Hmac::new(secret.as_bytes()));
        }
        if crate::settings().dev {
            return Some(Hmac::new(&dev_key()));
        }
        crate::http::log(format_args!("{NO_SECRET}"));
        None
    })
    .as_ref()
}

#[cfg(test)]
thread_local! {
    /// A test's thread has no secret, as a server without `WISP_SECRET`.
    pub(crate) static NO_KEY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Why a request that signs answers 500 without `WISP_SECRET`.
pub(crate) const NO_SECRET: &str = "wisp: signed cookies and tokens need a secret: set WISP_SECRET to at least 32 random characters (`openssl rand -hex 32` makes one), the same on every server of the app";

/// `WISP_SECRET_OLD`: the secret before `WISP_SECRET`, which signatures are
/// still checked with (nothing is signed with it). Keep it as long as the
/// cookies it signed last, then remove it: 30 days for a sign-in (a
/// session's age is in its value, so it ends then whatever signed it), 400
/// days for `set_signed_cookie`'s own.
fn old_key() -> Option<&'static Hmac> {
    static OLD: OnceLock<Option<Hmac>> = OnceLock::new();
    OLD.get_or_init(|| {
        let old = crate::settings().old_secret.as_ref()?;
        Some(Hmac::new(old.as_bytes()))
    })
    .as_ref()
}

fn dev_key() -> Vec<u8> {
    let Some(root) = ROOT.get() else {
        return random::<32>().to_vec();
    };
    let file = Path::new(root).join(".wisp").join("secret");
    if let Ok(text) = std::fs::read_to_string(&file)
        && text.trim().len() >= 32
    {
        keep_private(&file);
        return text.trim().as_bytes().to_vec();
    }
    let secret = hex(&random::<32>());
    let saved = std::fs::create_dir_all(file.parent().unwrap_or(Path::new(".")))
        .and_then(|()| save_private(&file, &secret));
    if let Err(e) = saved {
        crate::http::log(format_args!(
            "wisp: could not save a dev secret to {}: {e}; signed cookies will not survive a restart",
            file.display()
        ));
    }
    secret.into_bytes()
}

/// Writes `secret` to a file only its owner can read: with it, anyone can
/// sign in as anyone.
fn save_private(file: &Path, secret: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(file)?.write_all(secret.as_bytes())
}

/// A secret saved by an earlier version, or by hand, may be readable by
/// others: closed to them the next time it is read.
fn keep_private(file: &Path) {
    #[cfg(unix)]
    let _ = std::fs::set_permissions(file, std::os::unix::fs::PermissionsExt::from_mode(0o600));
    #[cfg(not(unix))]
    let _ = file;
}

/// A secret from the environment, such as an API key, and the HMAC it keys.
pub(crate) struct Secret {
    pub(crate) bytes: Box<[u8]>,
    hmac: Hmac,
}

/// `f` of the secret in the environment variable `var`; `None` when it is
/// unset or empty. Read once a thread: the environment stays as it was when
/// the app started.
pub(crate) fn secret<R>(var: &str, f: impl FnOnce(&Secret) -> R) -> Option<R> {
    thread_local! {
        static SECRETS: RefCell<Vec<(Box<str>, Option<Secret>)>> = const { RefCell::new(Vec::new()) };
    }
    SECRETS.with_borrow_mut(|all| {
        let at = match all.iter().position(|(n, _)| **n == *var) {
            Some(at) => at,
            None => {
                let s = crate::env(var).filter(|s| !s.is_empty()).map(|s| Secret {
                    hmac: Hmac::new(s.as_bytes()),
                    bytes: s.into_bytes().into(),
                });
                all.push((var.into(), s));
                all.len() - 1
            }
        };
        all[at].1.as_ref().map(f)
    })
}

/// `N` bytes from the host's `crypto.getRandomValues`.
#[cfg(target_arch = "wasm32")]
pub(crate) fn random<const N: usize>() -> [u8; N] {
    let mut out = [0u8; N];
    crate::edge::fill_random(&mut out);
    out
}

/// `N` unpredictable bytes: HMAC-SHA256 of a counter, under a key each
/// thread takes once from the OS's random source (see `seed`). HMAC is a
/// PRF: without the key, its output cannot be told from random, nor one
/// output guessed from others. About half a microsecond per 32 bytes.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn random<const N: usize>() -> [u8; N] {
    thread_local! {
        static RNG: RefCell<(Hmac, u64)> = RefCell::new((Hmac::new(&seed()), 0));
    }
    let mut out = [0u8; N];
    RNG.with_borrow_mut(|(key, n)| {
        for chunk in out.chunks_mut(32) {
            *n += 1;
            chunk.copy_from_slice(&key.sign(&[&n.to_le_bytes()])[..chunk.len()]);
        }
    });
    out
}

/// A key for `random`: 32 bytes of `/dev/urandom` where there is one, and
/// std's per-thread hash keys, which std takes from the OS's random source
/// on every platform (128 bits, read here through SipHash, the only way
/// without `unsafe`), hashed together. Either alone would do; a missing
/// `/dev/urandom` (a bare chroot) leaves the other.
#[cfg(not(target_arch = "wasm32"))]
fn seed() -> [u8; 32] {
    use std::hash::{BuildHasher, Hasher};
    let mut h = Sha256::new();
    #[cfg(unix)]
    {
        let mut os = [0u8; 32];
        let read = std::fs::File::open("/dev/urandom")
            .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut os));
        if read.is_ok() {
            h.update(&os);
        }
    }
    let keys = std::collections::hash_map::RandomState::new();
    for i in 0..4u64 {
        let mut s = keys.build_hasher();
        s.write_u64(i);
        h.update(&s.finish().to_le_bytes());
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    h.update(&nanos.to_le_bytes());
    h.finish()
}

/// HMAC-SHA256 of `message` under `key`, for checking a webhook's
/// signature by hand: `wisp::hex(&wisp::hmac_sha256(secret, body))`.
pub fn hmac_sha256(key: impl AsRef<[u8]>, message: impl AsRef<[u8]>) -> [u8; 32] {
    Hmac::new(key.as_ref()).sign(&[message.as_ref()])
}

/// Bytes as lowercase hex.
pub use wisp_shared::sha256::hex;

impl crate::Cx {
    /// Unless the request's header `header` signs its body with the secret
    /// in the environment variable `var`, a 401: how webhooks prove who
    /// sent them. `cx.need_signature("GITHUB_SECRET", "x-hub-signature-256")?`
    ///
    /// The signature is HMAC-SHA256, in hex (`sha256=` in front is fine, as
    /// GitHub sends it) or base64 (Shopify). Stripe's `t=…,v1=…` signs the
    /// time and the body, and is refused more than five minutes after `t`.
    /// Compared in constant time; an unset variable matches nothing.
    pub fn need_signature(&self, var: &str, header: &str) -> crate::Result {
        let sent = self.header(header).unwrap_or("");
        let stripe = sent.split(',').find_map(|p| p.trim().strip_prefix("t="));
        let ok = secret(var, |k| match stripe {
            Some(t) => {
                let fresh = t
                    .parse::<u64>()
                    .is_ok_and(|t| crate::unix_now().abs_diff(t) <= 300);
                let mac = k.hmac.sign(&[t.as_bytes(), b".", self.body()]);
                fresh
                    && sent
                        .split(',')
                        .filter_map(|p| p.trim().strip_prefix("v1="))
                        .any(|s| same_mac(s, &mac))
            }
            None => {
                let s = sent.trim();
                same_mac(
                    s.strip_prefix("sha256=").unwrap_or(s),
                    &k.hmac.sign(&[self.body()]),
                )
            }
        });
        if ok == Some(true) {
            return Ok(());
        }
        Err(crate::Error::new(401, "The signature does not match").with_code("bad_signature"))
    }
}

/// HMAC-SHA256 (RFC 2104) with one key: the hashes of its two padded
/// blocks are kept, which halves the work of each signature.
struct Hmac {
    inner: Sha256,
    outer: Sha256,
}

impl Hmac {
    fn new(key: &[u8]) -> Hmac {
        let mut k = [0u8; 64];
        if key.len() > 64 {
            k[..32].copy_from_slice(&sha256(&[key]));
        } else {
            k[..key.len()].copy_from_slice(key);
        }
        let padded = |byte: u8| {
            let mut h = Sha256::new();
            h.update(&k.map(|b| b ^ byte));
            h
        };
        Hmac {
            inner: padded(0x36),
            outer: padded(0x5c),
        }
    }

    /// The signature of the concatenation of `parts`.
    fn sign(&self, parts: &[&[u8]]) -> [u8; 32] {
        let mut inner = self.inner.clone();
        for p in parts {
            inner.update(p);
        }
        let mut outer = self.outer.clone();
        outer.update(&inner.finish());
        outer.finish()
    }
}

/// PBKDF2-HMAC-SHA256 (RFC 8018 §5.2) of `password` and `salt`: the first
/// 32 bytes of the key, all [`crate::password`] keeps. Each round is two
/// compressions: the key's padded blocks are hashed once, up front, and a
/// round's input (32 bytes, then SHA-256's padding for a message of 96)
/// fills exactly one block, kept as words, so nothing is copied, padded or
/// allocated in the loop.
pub(crate) fn pbkdf2(password: &[u8], salt: &[u8], rounds: u32) -> [u8; 32] {
    let key = Hmac::new(password);
    let first = key.sign(&[salt, &1u32.to_be_bytes()]);
    let mut u = [0u32; 8];
    for (w, b) in u.iter_mut().zip(first.as_chunks::<4>().0) {
        *w = u32::from_be_bytes(*b);
    }
    let mut sum = u;
    let mut block = [0u32; 16];
    block[8] = 0x8000_0000;
    block[15] = (64 + 32) * 8;
    for _ in 1..rounds {
        block[..8].copy_from_slice(&u);
        let mut inner = key.inner.state;
        compress_words(&mut inner, &block);
        block[..8].copy_from_slice(&inner);
        u = key.outer.state;
        compress_words(&mut u, &block);
        for (s, x) in sum.iter_mut().zip(u) {
            *s ^= x;
        }
    }
    let mut out = [0u8; 32];
    for (o, s) in out.as_chunks_mut::<4>().0.iter_mut().zip(sum) {
        *o = s.to_be_bytes();
    }
    out
}

/// SHA-1 (`ws.rs`) reads its input by the same blocks.
pub(crate) use wisp_shared::sha256::Blocks;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_vectors() {
        // RFC 4231 test cases 1, 2 and 6 (a key longer than a block).
        assert_eq!(
            hex(&Hmac::new(&[0x0b; 20]).sign(&[b"Hi There"])),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
        assert_eq!(
            hex(&Hmac::new(b"Jefe").sign(&[b"what do ya want for nothing?"])),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        let long = Hmac::new(&[0xaa; 131])
            .sign(&[b"Test Using Larger Than Block-Size Key - Hash Key First"]);
        assert_eq!(
            hex(&long),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn webhook_signatures() {
        // GitHub's documented example.
        assert_eq!(
            hex(&hmac_sha256("It's a Secret to Everybody", "Hello, World!")),
            "757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17"
        );
        // The secret: `CARGO_PKG_NAME`, which cargo sets for tests.
        let (var, secret, body) = ("CARGO_PKG_NAME", env!("CARGO_PKG_NAME"), "Hello, World!");
        let cx = |h: &str, v: &str| {
            crate::Cx::for_test(&format!("POST /h HTTP/1.1\r\n{h}: {v}\r\n\r\n{body}"), &[])
        };
        let mac = hmac_sha256(secret, body);
        let github = format!("sha256={}", hex(&mac));
        assert!(cx("x-sig", &github).need_signature(var, "x-sig").is_ok());
        assert!(
            cx("x-sig", &hex(&mac).to_uppercase())
                .need_signature(var, "x-sig")
                .is_ok()
        );
        let wrong = cx("x-sig", &github.replace('a', "b")).need_signature(var, "x-sig");
        assert_eq!(wrong.unwrap_err().status(), 401);
        assert!(
            cx("x-sig", &github)
                .need_signature("WISP_UNSET_SECRET", "x-sig")
                .is_err()
        );
        assert!(cx("x-other", &github).need_signature(var, "x-sig").is_err());
        for url in [false, true] {
            let mut b64 = String::new();
            base64(&mut b64, &mac, url);
            assert!(cx("x-sig", &b64).need_signature(var, "x-sig").is_ok());
        }
        let t = crate::unix_now();
        let v1 = hex(&hmac_sha256(secret, format!("{t}.{body}")));
        let stripe = format!("t={t},v1=00,v1={v1}");
        assert!(
            cx("stripe-signature", &stripe)
                .need_signature(var, "stripe-signature")
                .is_ok()
        );
        let old = format!("t={},v1={v1}", t - 600);
        assert!(
            cx("stripe-signature", &old)
                .need_signature(var, "stripe-signature")
                .is_err()
        );
    }

    #[test]
    fn pbkdf2_vectors() {
        // RFC 7914 §11 (the first 32 bytes of its 64), and the RFC 6070
        // inputs with SHA-256.
        for (password, salt, rounds, key) in [
            (
                &b"passwd"[..],
                &b"salt"[..],
                1,
                "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc",
            ),
            (
                b"password",
                b"salt",
                1,
                "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b",
            ),
            (
                b"password",
                b"salt",
                2,
                "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43",
            ),
            (
                b"password",
                b"salt",
                4096,
                "c5e478d59288c841aa530db6845c4c8d962893a001ce4e11a4963873aa98134a",
            ),
            (
                b"Password",
                b"NaCl",
                80000,
                "4ddcd8f60b98be21830cee5ef22701f9641a4418d04c0414aeff08876b34ab56",
            ),
        ] {
            assert_eq!(hex(&pbkdf2(password, salt, rounds)), key, "{rounds}");
        }
        // A password longer than a block is hashed first, as HMAC's key.
        let long = [b'p'; 100];
        let key = Hmac::new(&long);
        let mut u = key.sign(&[b"s", &1u32.to_be_bytes()]);
        let mut by_hand = u;
        for _ in 1..3 {
            u = key.sign(&[&u]);
            by_hand.iter_mut().zip(u).for_each(|(s, x)| *s ^= x);
        }
        assert_eq!(pbkdf2(&long, b"s", 3), by_hand);
    }

    #[test]
    fn random_bytes_differ() {
        assert_ne!(random::<32>(), random::<32>());
        // Longer than one HMAC, and across threads, which key their own.
        let long = random::<80>();
        assert_ne!(long[..32], long[32..64]);
        let other = std::thread::spawn(random::<32>).join().unwrap();
        assert_ne!(other, random::<32>());
        // Roughly half the bits set, over many bytes: no stuck output.
        let ones: u32 = (0..64)
            .flat_map(|_| random::<32>())
            .map(u8::count_ones)
            .sum();
        assert!((7_700..8_700).contains(&ones), "{ones} of 16384");
    }

    /// A signature from the secret before (`WISP_SECRET_OLD`) holds; one
    /// from any other key does not.
    #[test]
    fn signatures_by_hand_and_by_other_keys() {
        let mut mac = String::new();
        base64(&mut mac, &cookie_mac("user", "42").unwrap(), true);
        assert!(verify_cookie("user", "42", &mac));
        assert!(!verify_cookie("user", "43", &mac));
        assert!(!verify_cookie("users", "42", &mac), "the name is signed");
        let mut other = String::new();
        base64(&mut other, &hmac_sha256([7u8; 32], "user=42"), true);
        assert!(!verify_cookie("user", "42", &other));
    }

    /// Hostile signatures and webhook headers: never a panic, never a match.
    #[test]
    fn mangled_signatures_never_pass() {
        use crate::fuzz::{Rng, mutate};
        let mut rng = Rng::new(31);
        let mut good = String::new();
        base64(&mut good, &cookie_mac("session", "1.2").unwrap(), true);
        for _ in 0..20_000 {
            let mut b = good.clone().into_bytes();
            mutate(&mut rng, &mut b);
            let Ok(sent) = String::from_utf8(b) else {
                continue;
            };
            let mut back = [0u8; 32];
            let decoded = unbase64(&sent, &mut back);
            if decoded == Some(32) && Some(back) == cookie_mac("session", "1.2") {
                continue; // `=` padding added, or the other alphabet: the same bytes
            }
            assert!(!verify_cookie("session", "1.2", &sent), "{sent:?}");
            if !sent.contains(['\r', '\n']) {
                let head = format!("POST / HTTP/1.1\r\nx-sig: {sent}\r\n\r\nbody");
                let cx = crate::Cx::for_test(&head, &[]);
                assert!(cx.need_signature("CARGO_PKG_NAME", "x-sig").is_err());
            }
        }
    }
}
