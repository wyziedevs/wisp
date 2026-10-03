//! What the property tests of the parsers share: a seeded generator, so a
//! failure repeats, a mutator that breaks inputs the way hostile or buggy
//! clients do, and an app to parse requests for.

use crate::rt::RouteFacts;
use crate::{App, Asset, Cx, Error, Method, Out, Response};

pub use wisp_shared::rng::Rng;

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
/// `SMALL` bytes, and `/p/[x]` (2). One template, `TEMPLATE`, for the dev
/// endpoint to swap.
pub struct Fuzz;

pub const SMALL: usize = 64;

pub const TEMPLATE: (&str, u64) = ("src/routes/+page.wisp", 0xabc);

impl App for Fuzz {
    const ROOT: &'static str = ".";
    const CSS: Option<&'static str> = None;
    const ROUTES: &'static [RouteFacts] = &[
        RouteFacts::new(&[]),
        RouteFacts {
            body_limit: Some(SMALL),
            ..RouteFacts::new(&[])
        },
        RouteFacts::new(&["x"]),
    ];
    const TEMPLATES: &'static [(&'static str, u64)] = &[TEMPLATE];

    fn route(path: &str) -> Option<(usize, [&str; 8])> {
        let mut segs = [""; crate::rt::MAX_SEGS];
        Some(match crate::rt::split(path, &mut segs)? {
            [] => (0, [""; 8]),
            ["small"] => (1, [""; 8]),
            ["p", x] => (2, [*x, "", "", "", "", "", "", ""]),
            _ => return None,
        })
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
