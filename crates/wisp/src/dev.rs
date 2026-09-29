//! Dev-build support: hot-swapped template text, files read from disk, and
//! the loopback-only endpoint `wisp dev` talks to.
//!
//! Callers gate everything on `settings().dev` (see `WISP_DEV`), so a
//! release build can be run in dev mode too. Generated release code never
//! calls `chunk`.

use crate::App;
use crate::cx::Method;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

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
    swapped
        .iter()
        .find(|(id, _)| *id == t)
        .and_then(|(_, c)| c.get(i).copied())
        .unwrap_or(compiled)
}

/// Port of `wisp dev`'s event stream, if this process was started by it.
pub(crate) fn events_port() -> Option<u16> {
    if !crate::settings().dev {
        return None;
    }
    std::env::var("WISP_DEV_EVENTS").ok()?.parse().ok()
}

/// Under `wisp dev`, ends the process when `wisp dev` goes, however it
/// went: it holds our stdin, which reads as closed once it is gone. So a
/// killed `wisp dev` never leaves an app behind on its port.
pub(crate) fn exit_with_parent() {
    if events_port().is_none() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("wisp-dev-parent".into())
        .spawn(|| {
            let mut buf = [0u8; 64];
            while matches!(std::io::Read::read(&mut std::io::stdin(), &mut buf), Ok(n) if n > 0) {}
            std::process::exit(0);
        });
}

/// `/_wisp/dev/*`. Only loopback peers are answered.
pub(crate) fn endpoint<A: App>(
    method: Method,
    path: &str,
    body: &[u8],
    peer: SocketAddr,
) -> (u16, &'static str) {
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
    let id = A::TEMPLATES
        .iter()
        .position(|(p, _)| *p == path)
        .ok_or("unknown template")?;
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

/// One request in the dev log, lined up under `wisp dev`'s own lines: the
/// request, its status, the time it took, for a 5xx what went wrong, and
/// whether a handler blocked its thread, and the request's id if it has one. The status is yellow for a 4xx and
/// red for a 5xx, with the number beside it, so color is never the only sign.
pub(crate) fn log_request(
    method: &str,
    path: &str,
    status: u16,
    took: Duration,
    failure: Option<&str>,
    blocked: Option<Duration>,
    id: Option<&str>,
) {
    let paint = |code: &str, s: &str| {
        if color() {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    };
    let tint = match status {
        500.. => "31",
        400.. => "33",
        _ => "2",
    };
    let ms = format!("{:.3}ms", took.as_secs_f64() * 1000.0);
    let mut line = format!(
        "    {method} {path}  {}  {}",
        paint(tint, &status.to_string()),
        paint("2", &ms)
    );
    if let Some(id) = id {
        line.push_str("  ");
        line.push_str(&paint("2", id));
    }
    if let Some(f) = failure {
        line.push_str("\n      ");
        line.push_str(&paint("31", f));
    }
    if let Some(b) = blocked {
        // Its thread serves other connections too; they all waited.
        let note = format!(
            "! blocked its thread for {} ms, and every request on that thread waited. \
             Await instead of blocking: tokio::time::sleep, an async client, or \
             tokio::task::spawn_blocking for slow work.",
            b.as_millis()
        );
        line.push_str("\n      ");
        line.push_str(&paint("33", &note));
    }
    crate::http::log(format_args!("{line}"));
}

/// ANSI color on the dev log, by the same test the CLI uses: a terminal that
/// is known to take it. The old Windows console only does once a program
/// switches it on, which takes `unsafe`.
fn color() -> bool {
    static COLOR: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *COLOR.get_or_init(|| {
        use std::io::IsTerminal;
        let var = |k| std::env::var_os(k).is_some();
        let vt = !cfg!(windows) || var("WT_SESSION") || var("TERM_PROGRAM") || var("TERM");
        vt && !var("NO_COLOR") && std::io::stderr().is_terminal()
    })
}

/// A file for `path` from the project directory: `/_app/app.css` is the
/// built CSS (or `src/app.css`), anything else comes from `static/`.
pub(crate) fn read_file(root: &str, path: &str) -> Option<(Vec<u8>, String)> {
    let root = Path::new(root);
    let file = if path == "/_app/app.css" {
        let built = root.join(".wisp").join("app.css");
        if built.is_file() {
            built
        } else {
            root.join("src").join("app.css")
        }
    } else {
        root.join("static")
            .join(crate::http::safe_relative_path(path)?)
    };
    let bytes = std::fs::read(&file).ok()?; // also fails for directories
    let ext = file
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    Some((bytes, ext))
}

/// Whether `static/` had a file at `path` (a decoded URL path) when first
/// asked. A page's request checks this instead of the disk: a failed open
/// was most of a small page's time in dev (tens of µs on Windows). A file
/// added later is still found at any path that is not a page's.
pub(crate) fn listed(root: &str, path: &str) -> bool {
    static FILES: OnceLock<HashSet<String>> = OnceLock::new();
    FILES
        .get_or_init(|| {
            let mut files = HashSet::new();
            list(
                &Path::new(root).join("static"),
                &mut String::new(),
                0,
                &mut files,
            );
            files
        })
        .contains(path)
}

fn list(dir: &Path, url: &mut String, depth: usize, files: &mut HashSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let len = url.len();
        url.push('/');
        url.push_str(&entry.file_name().to_string_lossy());
        let path = entry.path();
        if !path.is_dir() {
            files.insert(url.clone());
        } else if depth < 32 {
            // Bounded: a symlink loop would recurse forever.
            list(&path, url, depth + 1, files);
        }
        url.truncate(len);
    }
}
