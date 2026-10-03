//! Base64 (RFC 4648), standard or URL-safe: the runtime's signed cookies,
//! password hashes and WebSocket handshakes, and the CLI's inlined wasm.

/// `bytes` as base64 onto `out`: URL-safe and unpadded when `url`, else
/// standard and padded.
pub fn encode(out: &mut String, bytes: &[u8], url: bool) {
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
pub fn decode(s: &str, out: &mut [u8]) -> Option<usize> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(bytes: &[u8], url: bool) -> String {
        let mut s = String::new();
        encode(&mut s, bytes, url);
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
            (b"fooba", "Zm9vYmE", "Zm9vYmE="),
            (b"foobar", "Zm9vYmFy", "Zm9vYmFy"),
            (&[0xfb, 0xff], "-_8", "+/8="),
        ] {
            assert_eq!(
                (b64(bytes, true), b64(bytes, false)),
                (url.into(), std.into())
            );
            for text in [url, std] {
                let mut out = [0; 8];
                let n = decode(text, &mut out).unwrap();
                assert_eq!(&out[..n], bytes);
            }
        }
        assert_eq!(b64(&[0; 32], true).len(), 43);
    }
}
