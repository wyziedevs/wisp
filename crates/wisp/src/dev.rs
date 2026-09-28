//! Dev-build support: hot-swapped template text, files read from disk, and
//! the loopback-only endpoint `wisp dev` talks to.
//!
//! Callers gate everything with `cfg!(debug_assertions)` rather than `#[cfg]`
//! so this code is type-checked in every build; release builds optimize it
//! away (generated release code never calls `chunk`).

use crate::cx::Method;
use crate::App;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// True once any template has been swapped; keeps `chunk` to one atomic
/// load until the first hot swap.
static SWAPPED_ANY: AtomicBool = AtomicBool::new(false);
/// (template id, its static text). Swapped text is leaked: a few KB per
/// save, and the dev server restarts on every Rust rebuild anyway.
static SWAPPED: RwLock<Vec<(usize, Vec<&'static str>)>> = RwLock::new(Vec::new());

/// Static text `i` of template `t`: the hot-swapped version if there is one.
#[inline]
pub fn chunk(t: usize, i: usize, compiled: &'static str) -> &'static str {
    if !SWAPPED_ANY.load(Ordering::Acquire) {
        return compiled;
    }
    let swapped = SWAPPED.read().unwrap_or_else(|e| e.into_inner());
    swapped.iter().find(|(id, _)| *id == t).and_then(|(_, c)| c.get(i).copied()).unwrap_or(compiled)
}

/// Port of `wisp dev`'s event stream, if this process was started by it.
pub(crate) fn events_port() -> Option<u16> {
    if !cfg!(debug_assertions) {
        return None;
    }
    std::env::var("WISP_DEV_EVENTS").ok()?.parse().ok()
}

/// `/_wisp/dev/*`. Only loopback peers are answered.
pub(crate) fn endpoint<A: App>(method: Method, path: &str, body: &[u8], peer: SocketAddr) -> (u16, &'static str) {
    if !peer.ip().is_loopback() {
        return (404, "Not Found");
    }
    match (method, path) {
        (Method::Post, "/_wisp/dev/swap") => match swap::<A>(body) {
            Ok(()) => (200, "swapped"),
            Err(e) => (409, e),
        },
        _ => (404, "Not Found"),
    }
}

/// Body: `path\nshape-hex\ncount\n` then per chunk `byte-length\n<bytes>`.
/// Refused unless the shape matches the compiled template exactly.
fn swap<A: App>(body: &[u8]) -> Result<(), &'static str> {
    fn line<'a>(rest: &mut &'a str) -> Result<&'a str, &'static str> {
        let (l, r) = rest.split_once('\n').ok_or("truncated request")?;
        *rest = r;
        Ok(l)
    }
    let mut rest = std::str::from_utf8(body).map_err(|_| "body is not UTF-8")?;
    let path = line(&mut rest)?;
    let shape = u64::from_str_radix(line(&mut rest)?, 16).map_err(|_| "bad shape")?;
    let count: usize = line(&mut rest)?.parse().map_err(|_| "bad count")?;
    let id = A::TEMPLATES.iter().position(|(p, _)| *p == path).ok_or("unknown template")?;
    if A::TEMPLATES[id].1 != shape {
        return Err("template shape changed; rebuild needed");
    }
    let mut chunks = Vec::with_capacity(count);
    for _ in 0..count {
        let len: usize = line(&mut rest)?.parse().map_err(|_| "bad length")?;
        if rest.len() < len || !rest.is_char_boundary(len) {
            return Err("bad length");
        }
        let (c, r) = rest.split_at(len);
        chunks.push(&*Box::leak(c.to_owned().into_boxed_str()));
        rest = r;
    }

    let mut swapped = SWAPPED.write().unwrap_or_else(|e| e.into_inner());
    match swapped.iter_mut().find(|(t, _)| *t == id) {
        Some(entry) => entry.1 = chunks,
        None => swapped.push((id, chunks)),
    }
    SWAPPED_ANY.store(true, Ordering::Release);
    Ok(())
}

/// A file for `path` from the project directory: `/_app/app.css` is the
/// built CSS (or `src/app.css`), anything else comes from `static/`.
pub(crate) fn read_file(root: &str, path: &str) -> Option<(Vec<u8>, String)> {
    let root = Path::new(root);
    let file = if path == "/_app/app.css" {
        let built = root.join(".wisp").join("app.css");
        if built.is_file() { built } else { root.join("src").join("app.css") }
    } else {
        root.join("static").join(crate::http::safe_relative_path(path)?)
    };
    let bytes = std::fs::read(&file).ok()?; // also fails for directories
    let ext = file.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    Some((bytes, ext))
}
