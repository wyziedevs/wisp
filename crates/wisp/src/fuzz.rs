//! What the property tests of the parsers share: a seeded generator, so a
//! failure repeats, a mutator that breaks inputs the way hostile or buggy
//! clients do, and an app to parse requests for.

use crate::{App, Asset, Cx, Error, Method, Out, Response};

/// xorshift64*: small, fast and good enough to find edge cases.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }

    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// From 0 to `n - 1`; 0 when `n` is 0.
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }

    pub fn one_in(&mut self, n: usize) -> bool {
        self.below(n) == 0
    }

    pub fn pick<T: Copy>(&mut self, from: &[T]) -> T {
        from[self.below(from.len())]
    }

    /// `n` bytes from `alphabet`, or any bytes if it is empty.
    pub fn bytes(&mut self, n: usize, alphabet: &[u8]) -> Vec<u8> {
        (0..n)
            .map(|_| {
                if alphabet.is_empty() {
                    self.next() as u8
                } else {
                    self.pick(alphabet)
                }
            })
            .collect()
    }

    /// Up to `max` bytes from `alphabet`.
    pub fn upto(&mut self, max: usize, alphabet: &[u8]) -> Vec<u8> {
        let n = self.below(max + 1);
        self.bytes(n, alphabet)
    }

    /// Text of up to `max` characters, ASCII mostly, with some that need
    /// escaping or take several bytes.
    pub fn text(&mut self, max: usize) -> String {
        const SOME: &[char] = &[
            'a', 'z', '0', ' ', '"', '\\', '/', '%', '+', '&', '=', ';', ',', '|', '<', '\n', '\r',
            '\t', '\0', '\u{7f}', 'é', 'ü', '€', '😀', '\u{2028}', '\u{fffd}',
        ];
        let n = self.below(max + 1);
        (0..n)
            .map(|_| {
                if self.one_in(3) {
                    self.pick(SOME)
                } else {
                    (b'a' + self.below(26) as u8) as char
                }
            })
            .collect()
    }
}

/// Bytes parsers split on or treat specially.
const SPECIAL: &[u8] = b"\r\n\0 \t:;,=&%+-\"\\{}[]0123456789abcdefABCDEF\x80\xff";

/// Breaks `b` a little: a byte changed, dropped, repeated or inserted, a
/// piece cut out or the end cut off.
pub fn mutate(rng: &mut Rng, b: &mut Vec<u8>) {
    for _ in 0..1 + rng.below(4) {
        let at = rng.below(b.len() + 1);
        match rng.below(7) {
            0 if at < b.len() => b[at] = rng.pick(SPECIAL),
            1 if at < b.len() => b[at] ^= 1 << rng.below(8),
            2 if at < b.len() => {
                b.remove(at);
            }
            3 => b.insert(at, rng.pick(SPECIAL)),
            4 if at < b.len() => {
                let end = (at + rng.below(16)).min(b.len());
                b.drain(at..end);
            }
            5 => b.truncate(at),
            _ => {
                let piece = b[at..(at + rng.below(8)).min(b.len())].to_vec();
                b.splice(at..at, piece);
            }
        }
    }
}

/// Routes: `/` (index 0), `/small` (1), which takes bodies of at most
/// `SMALL` bytes, and `/p/[x]` (2).
pub struct Fuzz;

pub const SMALL: usize = 64;

impl App for Fuzz {
    const ROOT: &'static str = ".";
    const CSS: Option<&'static str> = None;
    const PARAMS: &'static [&'static [&'static str]] = &[&[], &[], &["x"]];
    const TEMPLATES: &'static [(&'static str, u64)] = &[];

    fn route<'a>(_: &'a str, segs: &[&'a str]) -> Option<(usize, [&'a str; 8])> {
        Some(match segs {
            [] => (0, [""; 8]),
            ["small"] => (1, [""; 8]),
            ["p", x] => (2, [x, "", "", "", "", "", "", ""]),
            _ => return None,
        })
    }

    fn body_limit(route: usize) -> Option<usize> {
        (route == 1).then_some(SMALL)
    }

    fn shell() -> [&'static str; 3] {
        ["<html><head>", "</head><body>", "</body></html>"]
    }

    fn asset(_: &str) -> Option<&'static Asset> {
        None
    }

    async fn init() -> crate::Result<()> {
        Ok(())
    }

    async fn handle(route: Option<usize>, cx: &mut Cx, out: &mut Out) -> crate::Result<()> {
        crate::rt::hooked(cx);
        match (route, cx.method) {
            (None, _) => Err(Error::new(404, "Not Found")),
            (Some(0), Method::Get) => {
                crate::rt::respond(out, Response::text("Hello, World!"));
                Ok(())
            }
            (Some(_), _) => {
                if cx.route_param("x") == Some("id") {
                    cx.request_id();
                }
                let n = cx.form().iter().count();
                out.body.push_str(&n.to_string());
                Ok(())
            }
        }
    }

    async fn error(
        _: Option<usize>,
        cx: &mut Cx,
        out: &mut Out,
        status: u16,
        message: &str,
    ) -> crate::Result<()> {
        crate::rt::default_error(cx, out, status, message);
        Ok(())
    }
}
