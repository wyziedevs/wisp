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
use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

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
                Some((k, v)) if k.trim().eq_ignore_ascii_case("q") => {
                    v.trim().parse::<f32>().is_ok_and(|q| q > 0.0)
                }
                _ => true,
            })
    })
}

/// The gzip of an embedded `body`, made once and kept for good; `None`
/// when it did not get smaller.
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

/// A gzip file's header: deflate, no name, no time, unknown system.
const HEADER: [u8; 10] = [0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];

/// `data` as a gzip file.
pub(crate) fn gzip(data: &[u8]) -> Vec<u8> {
    let mut w = Bits::new(data.len());
    w.out.extend_from_slice(&HEADER);
    deflate(data, &mut w, true);
    w.put(0, 7); // pad to a byte
    let mut out = w.out;
    out.extend_from_slice(&crc32(data).to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}

/// A gzip file made a piece at a time: each piece is a block of its own
/// and a sync flush (an empty stored block), so what came so far decodes
/// whole; `end` closes the file. Matches reach back within a piece only.
pub(crate) struct Stream {
    crc: u32,
    len: u32,
    started: bool,
}

impl Stream {
    pub(crate) fn new() -> Stream {
        Stream {
            crc: !0,
            len: 0,
            started: false,
        }
    }

    pub(crate) fn piece(&mut self, data: &[u8]) -> Vec<u8> {
        let mut w = self.start(data.len());
        deflate(data, &mut w, false);
        w.put(0, 3); // not the last, stored
        if w.n > 0 {
            w.put(0, 8 - w.n); // to the byte
        }
        w.out.extend_from_slice(&[0, 0, 0xff, 0xff]); // empty
        self.crc = crc_add(self.crc, data);
        self.len = self.len.wrapping_add(data.len() as u32);
        w.out
    }

    pub(crate) fn end(mut self) -> Vec<u8> {
        let mut w = self.start(0);
        w.put(1, 1); // the last block
        w.put(1, 2); // fixed codes
        w.symbol(256);
        w.put(0, 7); // pad to a byte
        w.out.extend_from_slice(&(!self.crc).to_le_bytes());
        w.out.extend_from_slice(&self.len.to_le_bytes());
        w.out
    }

    fn start(&mut self, len: usize) -> Bits {
        let mut w = Bits::new(len);
        if !self.started {
            self.started = true;
            w.out.extend_from_slice(&HEADER);
        }
        w
    }
}

struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    fn new(len: usize) -> Bits {
        Bits {
            out: Vec::with_capacity(len / 3 + 32),
            acc: 0,
            n: 0,
        }
    }

    /// `bits` of `v`, low bit first.
    fn put(&mut self, v: u32, bits: u32) {
        self.acc |= (v as u64) << self.n;
        self.n += bits;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }

    /// The fixed Huffman code of literal/length symbol `s`: written
    /// high bit first, so reversed.
    fn symbol(&mut self, s: u32) {
        let (code, bits) = match s {
            0..=143 => (0x30 + s, 8),
            144..=255 => (0x190 + s - 144, 9),
            256..=279 => (s - 256, 7),
            _ => (0xc0 + s - 280, 8),
        };
        self.put(code.reverse_bits() >> (32 - bits), bits);
    }
}

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

const WINDOW: usize = 32768;
/// Earlier positions tried per byte: more finds longer matches, slower.
const CHAIN: usize = 32;
const HASH: usize = 1 << 15;

/// The hash of the three bytes at `i`.
fn hash(d: &[u8], i: usize) -> usize {
    let v = u32::from(d[i]) | u32::from(d[i + 1]) << 8 | u32::from(d[i + 2]) << 16;
    (v.wrapping_mul(0x9e37_79b1) >> 17) as usize
}

/// The matcher's tables: the latest position (plus one, 0 for none) of each
/// hash, and the one before it at each position of the window.
struct Chains {
    head: Vec<u32>,
    prev: Vec<u32>,
}

impl Chains {
    fn insert(&mut self, d: &[u8], i: usize) {
        if i + 3 <= d.len() {
            let h = hash(d, i);
            self.prev[i % WINDOW] = self.head[h];
            self.head[h] = i as u32 + 1;
        }
    }

    /// The longest match for the bytes at `i` (3 or more) and where it is.
    fn find(&self, d: &[u8], i: usize) -> Option<(usize, usize)> {
        if i + 3 > d.len() {
            return None;
        }
        let max = (d.len() - i).min(258);
        let (mut best, mut at) = (0, 0);
        let mut cand = self.head[hash(d, i)] as usize;
        for _ in 0..CHAIN {
            if cand == 0 || i - (cand - 1) > WINDOW {
                break;
            }
            let p = cand - 1;
            let n = d[p..p + max]
                .iter()
                .zip(&d[i..i + max])
                .take_while(|(a, b)| a == b)
                .count();
            if n > best {
                (best, at) = (n, p);
                if n == max {
                    break;
                }
            }
            // A slot the window has reused holds a later position: the
            // chain ends there.
            let next = self.prev[p % WINDOW] as usize;
            if next >= cand {
                break;
            }
            cand = next;
        }
        (best >= 3).then_some((best, at))
    }
}

/// One block of fixed Huffman codes, the `last` or not: literals and
/// (length, distance) matches.
fn deflate(d: &[u8], w: &mut Bits, last: bool) {
    w.put(last.into(), 1);
    w.put(1, 2); // fixed codes
    let mut chains = Chains {
        head: vec![0; HASH],
        prev: vec![0; WINDOW],
    };
    let mut i = 0;
    while i < d.len() {
        if let Some((len, at)) = chains.find(d, i) {
            let l = LEN_BASE.partition_point(|&b| b as usize <= len) - 1;
            w.symbol(257 + l as u32);
            w.put((len - LEN_BASE[l] as usize) as u32, LEN_EXTRA[l] as u32);
            let dist = i - at;
            let c = DIST_BASE.partition_point(|&b| b as usize <= dist) - 1;
            w.put((c as u32).reverse_bits() >> 27, 5);
            w.put((dist - DIST_BASE[c] as usize) as u32, DIST_EXTRA[c] as u32);
            for k in i..i + len {
                chains.insert(d, k);
            }
            i += len;
        } else {
            w.symbol(d[i].into());
            chains.insert(d, i);
            i += 1;
        }
    }
    w.symbol(256);
}

fn crc32(data: &[u8]) -> u32 {
    !crc_add(!0, data)
}

/// The CRC-32 register `c` after `data` (not inverted: start at `!0`, and
/// invert the end).
fn crc_add(c: u32, data: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut t = [0; 256];
        let mut n = 0;
        while n < 256 {
            let mut c = n as u32;
            let mut k = 0;
            while k < 8 {
                c = if c & 1 != 0 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
                k += 1;
            }
            t[n] = c;
            n += 1;
        }
        t
    };
    data.iter()
        .fold(c, |c, &b| TABLE[(c as u8 ^ b) as usize] ^ (c >> 8))
}

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
