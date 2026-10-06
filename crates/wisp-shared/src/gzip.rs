//! gzip with no dependency: fixed-Huffman deflate with a hash-chain
//! matcher, a little under a dense dynamic one, no tables to ship. The
//! runtime serves embedded files with it (`wisp::compress`) and `wisp build
//! --analyze` measures with it.

/// A gzip file's header: deflate, no name, no time, unknown system.
const HEADER: [u8; 10] = [0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 0xff];

/// `data` as a gzip file.
pub fn gzip(data: &[u8]) -> Vec<u8> {
    let mut w = Bits::new(data.len());
    w.out.extend_from_slice(&HEADER);
    deflate(data, &mut w, true);
    w.put(0, 7); // pad to a byte
    let mut out = w.out;
    out.extend_from_slice(&crc32(data).to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}

/// What a gzip file of [`gzip`] holds: fixed-Huffman and stored blocks
/// only, the CRC and length checked. `None` for anything else or damaged,
/// never a panic. Build-time-compressed text the edge wasm carries smaller
/// (the OpenAPI document) is unpacked with it once, on first use.
pub fn gunzip(gz: &[u8]) -> Option<Vec<u8>> {
    let body = gz.get(10..gz.len().checked_sub(8)?)?;
    if gz[..3] != HEADER[..3] {
        return None;
    }
    let mut pos = 0usize;
    let mut bits = |n: u32, to_byte: bool| -> Option<u32> {
        if to_byte {
            pos = pos.div_ceil(8) * 8;
        }
        let mut v = 0;
        for k in 0..n {
            v |= u32::from(body.get(pos / 8)? >> (pos % 8) & 1) << k;
            pos += 1;
        }
        Some(v)
    };
    let mut out: Vec<u8> = Vec::with_capacity(body.len() * 4);
    loop {
        let last = bits(1, false)? == 1;
        match bits(2, false)? {
            0 => {
                let len = bits(16, true)?;
                let nlen = bits(16, false)?;
                if len ^ 0xffff != nlen {
                    return None;
                }
                for _ in 0..len {
                    out.push(bits(8, false)? as u8);
                }
            }
            1 => loop {
                // Codes come high bit first: 7 bits, then more as the range says.
                let mut code = 0;
                for _ in 0..7 {
                    code = code << 1 | bits(1, false)?;
                }
                let sym = if code <= 0b0010111 {
                    code + 256
                } else {
                    code = code << 1 | bits(1, false)?;
                    match code {
                        0x30..=0xbf => code - 0x30,
                        0xc0..=0xc7 => code - 0xc0 + 280,
                        _ => (code << 1 | bits(1, false)?).checked_sub(0x190)? + 144,
                    }
                };
                match sym {
                    256 => break,
                    0..=255 => out.push(sym as u8),
                    s => {
                        let l = (s - 257) as usize;
                        let len =
                            *LEN_BASE.get(l)? as usize + bits(LEN_EXTRA[l].into(), false)? as usize;
                        let mut c = 0;
                        for _ in 0..5 {
                            c = c << 1 | bits(1, false)?;
                        }
                        let c = c as usize;
                        let dist = *DIST_BASE.get(c)? as usize
                            + bits(DIST_EXTRA[c].into(), false)? as usize;
                        let from = out.len().checked_sub(dist)?;
                        for k in from..from + len {
                            out.push(out[k]);
                        }
                    }
                }
            },
            _ => return None,
        }
        if last {
            break;
        }
    }
    let tail = &gz[gz.len() - 8..];
    let ok =
        tail[..4] == crc32(&out).to_le_bytes() && tail[4..] == (out.len() as u32).to_le_bytes();
    ok.then_some(out)
}

/// A gzip file made a piece at a time: each piece is a block of its own
/// and a sync flush (an empty stored block), so what came so far decodes
/// whole; `end` closes the file. Matches reach back within a piece only.
pub struct Stream {
    crc: u32,
    len: u32,
    started: bool,
}

impl Stream {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Stream {
        Stream {
            crc: !0,
            len: 0,
            started: false,
        }
    }

    pub fn piece(&mut self, data: &[u8]) -> Vec<u8> {
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

    pub fn end(mut self) -> Vec<u8> {
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

pub const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
pub const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
pub const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
pub const DIST_EXTRA: [u8; 30] = [
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

pub fn crc32(data: &[u8]) -> u32 {
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
