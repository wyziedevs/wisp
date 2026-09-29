//! Signed cookies: HMAC-SHA256 over the cookie's name and value, with the
//! app's secret.
//!
//! SHA-256 and HMAC are written here rather than pulled in, which keeps the
//! runtime at two dependencies. Both are short, fixed algorithms with
//! published test vectors (FIPS 180-4, RFC 4231), checked below. Nothing is
//! encrypted: a signed cookie's value is readable, only not forgeable.

use std::path::Path;
use std::sync::OnceLock;

/// The signature of cookie `name` holding `value`: base64url, no padding.
pub(crate) fn cookie_mac(name: &str, value: &str) -> String {
    base64url(&key().sign(&[name.as_bytes(), b"=", value.as_bytes()]))
}

/// Whether `mac` is the signature of cookie `name` holding `value`, in time
/// that does not depend on where they differ.
pub(crate) fn verify_cookie(name: &str, value: &str, mac: &str) -> bool {
    crate::secure_eq(cookie_mac(name, value), mac)
}

/// The project directory, for the dev secret. Set when the server starts.
pub(crate) static ROOT: OnceLock<&'static str> = OnceLock::new();

/// `WISP_SECRET`; in dev builds without it, a secret kept in the project's
/// `.wisp/secret`, so signed cookies survive restarts. A release build
/// without one cannot sign: the panic is the request's 500, with the reason.
fn key() -> &'static Hmac {
    static KEY: OnceLock<Hmac> = OnceLock::new();
    KEY.get_or_init(|| {
        if let Some(secret) = &crate::settings().secret {
            return Hmac::new(secret.as_bytes());
        }
        if cfg!(debug_assertions) {
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
        return text.trim().as_bytes().to_vec();
    }
    let secret: String = random::<32>().iter().map(|b| format!("{b:02x}")).collect();
    let saved = std::fs::create_dir_all(file.parent().unwrap_or(Path::new(".")))
        .and_then(|()| std::fs::write(&file, &secret));
    if let Err(e) = saved {
        crate::http::log(format_args!(
            "wisp: could not save a dev secret to {}: {e}; signed cookies will not survive a restart",
            file.display()
        ));
    }
    secret.into_bytes()
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

fn base64url(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let n = group
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for k in 0..group.len() + 1 {
            out.push(ABC[(n >> (18 - 6 * k) & 63) as usize] as char);
        }
    }
    out
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

/// SHA-256, FIPS 180-4.
#[derive(Clone)]
struct Sha256 {
    state: [u32; 8],
    block: [u8; 64],
    filled: usize,
    total: u64,
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
            block: [0; 64],
            filled: 0,
            total: 0,
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.total += data.len() as u64;
        while !data.is_empty() {
            let n = data.len().min(64 - self.filled);
            self.block[self.filled..self.filled + n].copy_from_slice(&data[..n]);
            self.filled += n;
            data = &data[n..];
            if self.filled == 64 {
                self.compress();
                self.filled = 0;
            }
        }
    }

    fn finish(mut self) -> [u8; 32] {
        let bits = self.total * 8;
        self.update(&[0x80]);
        let zeros = (120 - self.filled) % 64;
        self.update(&[0; 64][..zeros]);
        self.update(&bits.to_be_bytes());
        let mut out = [0u8; 32];
        for (o, s) in out.chunks_mut(4).zip(self.state) {
            o.copy_from_slice(&s.to_be_bytes());
        }
        out
    }

    fn compress(&mut self) {
        let mut w = [0u32; 64];
        for (i, word) in self.block.chunks(4).enumerate() {
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
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
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
        for (s, v) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *s = s.wrapping_add(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|b| format!("{b:02x}")).collect()
    }

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

    #[test]
    fn base64url_encoding() {
        assert_eq!(base64url(b""), "");
        assert_eq!(base64url(b"f"), "Zg");
        assert_eq!(base64url(b"fo"), "Zm8");
        assert_eq!(base64url(b"foo"), "Zm9v");
        assert_eq!(base64url(b"foob"), "Zm9vYg");
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
        assert_eq!(base64url(&[0; 32]).len(), 43);
    }

    #[test]
    fn random_bytes_differ() {
        assert_ne!(random::<32>(), random::<32>());
    }
}
