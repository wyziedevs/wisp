//! gzip for embedded files: `wisp.js`, the app's css, client modules and
//! `static/`, which never change while the server runs, so each is
//! compressed once (the first time a client that takes gzip asks) and the
//! copy is sent from then on. Pages and API answers are made per request
//! and stay as they are: a proxy or CDN in front compresses those better.
//! A streamed page (`{#await}`, see `tail.rs`) is the exception, since a
//! proxy may hold a stream back to compress it: it is compressed a piece at
//! a time ([`Stream`]), each piece flushed so the browser shows it at once.
//!
//! The compressor is fixed-Huffman deflate with a hash-chain matcher: a
//! little under a dense dynamic one, no dependency, no tables to ship.

use crate::cx::Cx;
use crate::http::{Body, Reply};
use std::borrow::Cow;
#[cfg(not(target_arch = "wasm32"))]
use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::OnceLock;
use std::sync::RwLock;

/// Smaller than this is not worth a header and a decode.
const MIN: usize = 256;

/// `reply`, a 200 with an embedded file, as gzip when the client takes it.
/// Types that are binary or compressed already are left alone.
pub(crate) fn apply(cx: &Cx, reply: &mut Reply) {
    let Body::Static(body) = reply.body else {
        return;
    };
    if body.len() < MIN || !text(reply.headers.first().map_or("", |h| &h.1)) {
        return;
    }
    reply
        .headers
        .push((Cow::Borrowed("vary"), Cow::Borrowed("accept-encoding")));
    if !wanted(cx) {
        return;
    }
    if let Some(gz) = copy(body) {
        reply.body = Body::Static(gz);
        reply
            .headers
            .push((Cow::Borrowed("content-encoding"), Cow::Borrowed("gzip")));
    }
}

/// Whether the request takes gzip.
pub(crate) fn wanted(cx: &Cx) -> bool {
    cx.header("accept-encoding").is_some_and(takes_gzip)
}

/// Whether a `content-type` is worth compressing.
fn text(ct: &str) -> bool {
    ct.starts_with("text/")
        || ["json", "javascript", "xml", "svg", "wasm"]
            .iter()
            .any(|t| ct.contains(t))
}

/// Whether an `accept-encoding` value lists gzip, and not as `q=0`.
fn takes_gzip(value: &str) -> bool {
    value.split(',').any(|part| {
        let mut p = part.split(';');
        let name = p.next().unwrap_or("").trim();
        (name.eq_ignore_ascii_case("gzip") || name.eq_ignore_ascii_case("x-gzip"))
            && p.all(|q| match q.trim().split_once('=') {
                Some((k, v)) if k.trim().eq_ignore_ascii_case("q") => positive(v.trim()),
                _ => true,
            })
    })
}

/// Whether a `q` value is above 0.
#[cfg(not(target_arch = "wasm32"))]
fn positive(q: &str) -> bool {
    q.parse::<f32>().is_ok_and(|q| q > 0.0)
}

/// Whether a `q` value is above 0: digits and at most one `.`, one of them
/// not 0, as every client writes it (`1`, `0.5`, `0.001`). The float parser
/// is 5 KB of wasm; it also took `1e-3` and `+1`, which no client sends.
#[cfg(target_arch = "wasm32")]
fn positive(q: &str) -> bool {
    let digits = q.bytes().filter(u8::is_ascii_digit).count();
    let dots = q.bytes().filter(|&b| b == b'.').count();
    digits > 0
        && digits + dots == q.len()
        && dots <= 1
        && q.bytes().any(|b| matches!(b, b'1'..=b'9'))
}

/// The gzip of an embedded `body`, made once and kept for good; `None`
/// when it did not get smaller.
#[cfg(target_arch = "wasm32")]
fn copy(body: &'static [u8]) -> Option<&'static [u8]> {
    // A few files, scanned: no hash table in the wasm.
    type Made = RwLock<crate::edge::Ids<(usize, usize), Option<&'static [u8]>>>;
    static MADE: Made = RwLock::new(crate::edge::Ids::new());
    let key = (body.as_ptr() as usize, body.len());
    if let Some(&kept) = MADE.read().ok()?.get(&key) {
        return kept;
    }
    let gz = gzip(body);
    let gz = (gz.len() < body.len()).then(|| &*Box::leak(gz.into_boxed_slice()));
    let mut made = MADE.write().ok()?;
    match made.get(&key) {
        Some(&kept) => kept,
        None => {
            made.insert(key, gz);
            gz
        }
    }
}

/// The gzip of an embedded `body`, made once and kept for good; `None`
/// when it did not get smaller.
#[cfg(not(target_arch = "wasm32"))]
fn copy(body: &'static [u8]) -> Option<&'static [u8]> {
    type Made = RwLock<HashMap<(usize, usize), Option<&'static [u8]>>>;
    static MADE: OnceLock<Made> = OnceLock::new();
    let made = MADE.get_or_init(Default::default);
    let key = (body.as_ptr() as usize, body.len());
    if let Some(&kept) = made.read().ok()?.get(&key) {
        return kept;
    }
    // Compressed with no lock held: two first requests may both do it (one
    // copy is kept), but no other file waits for it.
    let gz = gzip(body);
    let gz = (gz.len() < body.len()).then(|| &*Box::leak(gz.into_boxed_slice()));
    *made.write().ok()?.entry(key).or_insert(gz)
}

#[cfg(test)]
use wisp_shared::gzip::{DIST_BASE, DIST_EXTRA, LEN_BASE, LEN_EXTRA, crc32};
pub(crate) use wisp_shared::gzip::{Stream, gzip};

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A reference decoder for the gzip files this writes (fixed-Huffman
    /// and stored blocks): the tests do not trust the compressor to check
    /// itself.
    pub(crate) fn inflate(gz: &[u8]) -> Vec<u8> {
        let (out, last) = blocks(&gz[..gz.len() - 8]);
        assert!(last, "ends with the last block");
        out
    }

    /// What the blocks of a gzip file's start `gz` decode to, as far as it
    /// goes (a stream cut after a sync flush), and whether the last came.
    pub(crate) fn blocks(gz: &[u8]) -> (Vec<u8>, bool) {
        assert_eq!(&gz[..3], &[0x1f, 0x8b, 8]);
        let body = &gz[10..];
        let mut pos = 0;
        let bit = |pos: &mut usize, n: u32| {
            let mut v = 0;
            for k in 0..n {
                let b = (body[*pos / 8] >> (*pos % 8)) & 1;
                v |= u32::from(b) << k;
                *pos += 1;
            }
            v
        };
        let mut out: Vec<u8> = Vec::new();
        while pos + 3 <= body.len() * 8 {
            let last = bit(&mut pos, 1) == 1;
            match bit(&mut pos, 2) {
                0 => {
                    pos = pos.div_ceil(8) * 8;
                    let (len, nlen) = (bit(&mut pos, 16), bit(&mut pos, 16));
                    assert_eq!(len ^ 0xffff, nlen);
                    for _ in 0..len {
                        out.push(bit(&mut pos, 8) as u8);
                    }
                }
                1 => loop {
                    // Codes come high bit first: 7 bits, then more as the range says.
                    let mut code = 0;
                    for _ in 0..7 {
                        code = code << 1 | bit(&mut pos, 1);
                    }
                    let sym = if code <= 0b0010111 {
                        code + 256
                    } else {
                        code = code << 1 | bit(&mut pos, 1);
                        if (0x30..=0xbf).contains(&code) {
                            code - 0x30
                        } else if (0xc0..=0xc7).contains(&code) {
                            code - 0xc0 + 280
                        } else {
                            code = code << 1 | bit(&mut pos, 1);
                            code - 0x190 + 144
                        }
                    };
                    match sym {
                        256 => break,
                        0..=255 => out.push(sym as u8),
                        s => {
                            let l = (s - 257) as usize;
                            let len =
                                LEN_BASE[l] as usize + bit(&mut pos, LEN_EXTRA[l].into()) as usize;
                            let mut c = 0;
                            for _ in 0..5 {
                                c = c << 1 | bit(&mut pos, 1);
                            }
                            let dist = DIST_BASE[c as usize] as usize
                                + bit(&mut pos, DIST_EXTRA[c as usize].into()) as usize;
                            for _ in 0..len {
                                out.push(out[out.len() - dist]);
                            }
                        }
                    }
                },
                t => panic!("block type {t}"),
            }
            if last {
                return (out, true);
            }
        }
        (out, false)
    }

    fn round(data: &[u8]) -> usize {
        let gz = gzip(data);
        assert_eq!(inflate(&gz), data);
        let n = gz.len();
        assert_eq!(&gz[n - 8..n - 4], &crc32(data).to_le_bytes());
        assert_eq!(&gz[n - 4..], &(data.len() as u32).to_le_bytes());
        n
    }

    #[test]
    fn a_stream_decodes_after_each_piece() {
        let (a, b) = (b"<p>pending</p>".repeat(30), b"<p>answer</p>".repeat(5));
        let mut z = Stream::new();
        let first = z.piece(&a);
        assert_eq!(blocks(&first), (a.clone(), false));
        let mut all = first;
        all.extend(z.piece(&b));
        all.extend(z.piece(b""));
        all.extend(z.end());
        let whole = [a.as_slice(), &b].concat();
        assert_eq!(inflate(&all), whole);
        let n = all.len();
        assert_eq!(&all[n - 8..n - 4], &crc32(&whole).to_le_bytes());
        assert_eq!(&all[n - 4..], &(whole.len() as u32).to_le_bytes());
    }

    #[test]
    fn crc_is_the_standard_one() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn it_round_trips() {
        round(b"");
        round(b"a");
        round(b"abc");
        round(&[0; 100_000]); // overlapping matches, the longest length
        let text = "function wisp() { return document.querySelector('.card'); }\n".repeat(500);
        assert!(round(text.as_bytes()) < text.len() / 20);
        // Bytes with no pattern: every literal code, 8 and 9 bits.
        let mut x = 12345u32;
        let noise: Vec<u8> = (0..70_000)
            .map(|_| {
                x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
                (x >> 16) as u8
            })
            .collect();
        round(&noise);
    }

    #[test]
    fn matches_reach_across_the_window() {
        let mut d = vec![7u8; 40_000];
        d[..10].copy_from_slice(b"0123456789");
        d.extend_from_slice(b"0123456789");
        round(&d);
    }

    #[test]
    fn accept_encoding() {
        assert!(takes_gzip("gzip, deflate, br"));
        assert!(takes_gzip("br;q=1, gzip;q=0.5"));
        assert!(!takes_gzip("gzip;q=0"));
        assert!(!takes_gzip("gzip;q=0.0, br"));
        assert!(!takes_gzip("gzip;Q=0"), "the parameter is any case");
        assert!(!takes_gzip("gzip; q=0.000"));
        assert!(takes_gzip("GZIP;q=0.001"));
        assert!(!takes_gzip("br, identity"));
    }

    #[test]
    fn only_text_is_compressed() {
        assert!(text("text/css; charset=utf-8"));
        assert!(text("application/json"));
        assert!(text("image/svg+xml"));
        assert!(!text("image/png"));
        assert!(!text("font/woff2"));
    }
}
