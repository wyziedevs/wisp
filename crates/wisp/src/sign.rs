//! Signed cookies: HMAC-SHA256 over the cookie's name and value, with the
//! app's secret.
//!
//! SHA-256 and HMAC are written here rather than pulled in, which keeps the
//! runtime at two dependencies. Both are short, fixed algorithms with
//! published test vectors (FIPS 180-4, RFC 4231), checked below. Nothing is
//! encrypted: a signed cookie's value is readable, only not forgeable.

use crate::cx::hex_digit;
use std::cell::RefCell;
use std::path::Path;
use std::sync::OnceLock;

/// The signature of cookie `name` holding `value`.
pub(crate) fn cookie_mac(name: &str, value: &str) -> [u8; 32] {
    key().sign(&[name.as_bytes(), b"=", value.as_bytes()])
}

/// Whether `mac` is the signature of cookie `name` holding `value`, in time
/// that does not depend on where they differ.
pub(crate) fn verify_cookie(name: &str, value: &str, mac: &str) -> bool {
    same_mac(mac, &cookie_mac(name, value))
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
/// none to sign with: the panic is the request's 500, with the reason.
fn key() -> &'static Hmac {
    static KEY: OnceLock<Hmac> = OnceLock::new();
    KEY.get_or_init(|| {
        if let Some(secret) = &crate::settings().secret {
            return Hmac::new(secret.as_bytes());
        }
        if crate::settings().dev {
            return Hmac::new(&dev_key());
        }
        panic!(
            "signed cookies need a secret: set WISP_SECRET to at least 32 random characters \
             (`openssl rand -hex 32` makes one), the same on every server of the app"
        )
    })
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

/// `N` unpredictable bytes. std seeds every `RandomState` from the OS's
/// random source (once per thread, then steps it); SipHash of those keys
/// cannot be told from random by anyone who does not have them.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn random<const N: usize>() -> [u8; N] {
    use std::hash::{BuildHasher, Hasher};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut out = [0u8; N];
    for (i, chunk) in out.chunks_mut(8).enumerate() {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_usize(i);
        h.write_u128(nanos);
        chunk.copy_from_slice(&h.finish().to_le_bytes()[..chunk.len()]);
    }
    out
}

/// `bytes` as base64 onto `out`: URL-safe and unpadded when `url`, else
/// standard and padded.
pub(crate) fn base64(out: &mut String, bytes: &[u8], url: bool) {
    let abc: &[u8; 64] = if url {
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
    } else {
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
    };
    for group in bytes.chunks(3) {
        let n = group
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for k in 0..4 {
            if k <= group.len() {
                out.push(abc[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else if !url {
                out.push('=');
            }
        }
    }
}

/// Base64, standard or URL-safe, padded or not, decoded into `out`: how
/// many bytes, or `None` for anything else or more than fits.
pub(crate) fn unbase64(s: &str, out: &mut [u8]) -> Option<usize> {
    let (mut bits, mut n, mut len) = (0u32, 0, 0);
    for b in s.trim_end_matches('=').bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        };
        bits = bits << 6 | v as u32;
        n += 6;
        if n >= 8 {
            n -= 8;
            *out.get_mut(len)? = (bits >> n) as u8;
            len += 1;
        }
    }
    (n < 6).then_some(len)
}

/// HMAC-SHA256 of `message` under `key`, for checking a webhook's
/// signature by hand: `wisp::hex(&wisp::hmac_sha256(secret, body))`.
pub fn hmac_sha256(key: impl AsRef<[u8]>, message: impl AsRef<[u8]>) -> [u8; 32] {
    Hmac::new(key.as_ref()).sign(&[message.as_ref()])
}

/// Bytes as lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(DIGITS[(b >> 4) as usize] as char);
        s.push(DIGITS[(b & 15) as usize] as char);
    }
    s
}

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

fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finish()
}

/// The input of SHA-1 and SHA-256, in the 64-byte blocks they compress,
/// and the padding that ends it (FIPS 180-4 §5.1.1).
#[derive(Clone)]
pub(crate) struct Blocks {
    block: [u8; 64],
    filled: usize,
    total: u64,
}

impl Blocks {
    pub(crate) const fn new() -> Blocks {
        Blocks {
            block: [0; 64],
            filled: 0,
            total: 0,
        }
    }

    pub(crate) fn update(&mut self, mut data: &[u8], compress: &mut impl FnMut(&[u8; 64])) {
        self.total += data.len() as u64;
        while !data.is_empty() {
            let n = data.len().min(64 - self.filled);
            self.block[self.filled..self.filled + n].copy_from_slice(&data[..n]);
            self.filled += n;
            data = &data[n..];
            if self.filled == 64 {
                compress(&self.block);
                self.filled = 0;
            }
        }
    }

    /// Pads the input, which ends it.
    pub(crate) fn finish(mut self, compress: &mut impl FnMut(&[u8; 64])) {
        let bits = self.total * 8;
        self.update(&[0x80], compress);
        let zeros = (120 - self.filled) % 64;
        self.update(&[0; 64][..zeros], compress);
        self.update(&bits.to_be_bytes(), compress);
    }
}

/// SHA-256, FIPS 180-4.
#[derive(Clone)]
struct Sha256 {
    state: [u32; 8],
    blocks: Blocks,
}

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

impl Sha256 {
    fn new() -> Sha256 {
        let state = [
            0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
            0x5be0cd19,
        ];
        Sha256 {
            state,
            blocks: Blocks::new(),
        }
    }

    fn update(&mut self, data: &[u8]) {
        let state = &mut self.state;
        self.blocks.update(data, &mut |b| compress(state, b));
    }

    fn finish(mut self) -> [u8; 32] {
        let state = &mut self.state;
        self.blocks.finish(&mut |b| compress(state, b));
        let mut out = [0u8; 32];
        for (o, s) in out.chunks_mut(4).zip(self.state) {
            o.copy_from_slice(&s.to_be_bytes());
        }
        out
    }
}

/// SHA-256's compression of one block into `state`.
fn compress(state: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 64];
    for (i, word) in block.chunks(4).enumerate() {
        w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ (!e & g);
        let t1 = h
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        (h, g, f, e, d, c, b, a) = (g, f, e, d.wrapping_add(t1), c, b, a, t1.wrapping_add(t2));
    }
    for (s, v) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *s = s.wrapping_add(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_vectors() {
        assert_eq!(
            hex(&sha256(&[b""])),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(&[b"abc"])),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let two_blocks = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        assert_eq!(
            hex(&sha256(&[two_blocks])),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(
            hex(&sha256(&[&two_blocks[..10], &two_blocks[10..]])),
            hex(&sha256(&[two_blocks]))
        );
        let million = vec![b'a'; 1_000_000];
        assert_eq!(
            hex(&sha256(&[&million])),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

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

    fn b64(bytes: &[u8], url: bool) -> String {
        let mut s = String::new();
        base64(&mut s, bytes, url);
        s
    }

    #[test]
    fn base64_both_ways() {
        for (bytes, url, std) in [
            (&b""[..], "", ""),
            (b"f", "Zg", "Zg=="),
            (b"fo", "Zm8", "Zm8="),
            (b"foo", "Zm9v", "Zm9v"),
            (b"foob", "Zm9vYg", "Zm9vYg=="),
            (&[0xfb, 0xff], "-_8", "+/8="),
        ] {
            assert_eq!(
                (b64(bytes, true), b64(bytes, false)),
                (url.into(), std.into())
            );
            for text in [url, std] {
                let mut out = [0; 8];
                let n = unbase64(text, &mut out).unwrap();
                assert_eq!(&out[..n], bytes);
            }
        }
        assert_eq!(b64(&[0; 32], true).len(), 43);
    }

    #[test]
    fn webhook_signatures() {
        // GitHub's documented example.
        assert_eq!(
            hex(&hmac_sha256("It's a Secret to Everybody", "Hello, World!")),
            "757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17"
        );
        // The secret: `CARGO_PKG_NAME`, which cargo sets for tests.
        let (var, secret, body) = ("CARGO_PKG_NAME", "wisp", "Hello, World!");
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
            let b64 = b64(&mac, url);
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
    fn random_bytes_differ() {
        assert_ne!(random::<32>(), random::<32>());
    }
}
