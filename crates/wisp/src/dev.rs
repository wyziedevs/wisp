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
/// (template id, its shape now, its static text). Swapped text is leaked:
/// a few KB per save, and the dev server restarts on every Rust rebuild
/// anyway.
type Swapped = Vec<(usize, u64, Vec<&'static str>)>;
static SWAPPED: RwLock<Swapped> = RwLock::new(Vec::new());
/// Browser modules `wisp dev` swapped in: (path, URL, source), leaked the
/// same way, and whether there is any.
type Modules = Vec<(String, &'static str, &'static str)>;
static MODULES: RwLock<Modules> = RwLock::new(Vec::new());
static MODULES_ANY: AtomicBool = AtomicBool::new(false);

/// Static text `i` of template `t`: the hot-swapped version if there is one.
#[inline]
pub fn chunk(t: usize, i: usize, compiled: &'static str) -> &'static str {
    if !SWAPPED_ANY.load(Ordering::Acquire) {
        return compiled;
    }
    let swapped = SWAPPED.read().unwrap_or_else(|e| e.into_inner());
    swapped
        .iter()
        .find(|(id, ..)| *id == t)
        .and_then(|(_, _, c)| c.get(i).copied())
        .unwrap_or(compiled)
}

/// Whether templates mark what they render (`<!--w:file-->` … `<!--/w:file-->`),
/// for `wisp dev` to morph one file's part alone: under `wisp dev`, and in
/// browser tests, which swap as it does.
pub fn marks() -> bool {
    static MARKS: OnceLock<bool> = OnceLock::new();
    MARKED.load(Ordering::Relaxed) || *MARKS.get_or_init(|| events_port().is_some())
}
static MARKED: AtomicBool = AtomicBool::new(false);

/// Templates mark what they render from now on (see [`marks`]).
#[cfg(feature = "browser")]
pub(crate) fn mark() {
    MARKED.store(true, Ordering::Relaxed);
}

/// The source `wisp dev` swapped in for the module at `path`, if any.
#[cfg(debug_assertions)]
pub(crate) fn module(path: &str) -> Option<&'static str> {
    if !MODULES_ANY.load(Ordering::Acquire) {
        return None;
    }
    let modules = MODULES.read().unwrap_or_else(|e| e.into_inner());
    modules.iter().find(|m| m.0 == path).map(|m| m.2)
}

/// The URL pages name module `m` by: the swapped-in one's, in a debug
/// build, so a reload loads that and not the old one a cache keeps.
#[inline]
pub(crate) fn url(m: &'static crate::ClientModule) -> &'static str {
    #[cfg(debug_assertions)]
    if MODULES_ANY.load(Ordering::Acquire) {
        let modules = MODULES.read().unwrap_or_else(|e| e.into_inner());
        if let Some(swapped) = modules.iter().find(|s| s.0 == m.path) {
            return swapped.1;
        }
    }
    m.url
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

/// `/_wisp/dev/*`. Only loopback peers are answered, and only by a debug
/// build: a release build's pages never read swapped text, so there it is
/// nothing to anyone, `WISP_DEV=on` or not.
pub(crate) fn endpoint<A: App>(
    method: Method,
    path: &str,
    body: &[u8],
    peer: SocketAddr,
) -> (u16, &'static str) {
    if !cfg!(debug_assertions) || !peer.ip().is_loopback() {
        return (404, "Not Found");
    }
    match (method, path) {
        (Method::Post, "/_wisp/dev/swap") => match swap::<A>(body) {
            Ok(()) => (200, "swapped"),
            Err(e) => (409, e),
        },
        (Method::Post, "/_wisp/dev/module") => match swap_module::<A>(body) {
            Ok(()) => (200, "swapped"),
            Err(e) => (409, e),
        },
        _ => (404, "Not Found"),
    }
}

/// `POST /_wisp/dev/open`, body `file\nline` (`file` from the project
/// root, `/`-separated): opens the project's file at that line, for the
/// devtools and the workshop. Only a loopback peer that sends `x-wisp-dev`
/// is answered: a page of another site cannot send that header without
/// asking first (CORS), and is never told yes, so no site can open files.
#[cfg(debug_assertions)]
pub(crate) fn open(root: &str, cx: &crate::Cx) -> (u16, &'static str) {
    if !cx.peer().ip().is_loopback() || cx.header("x-wisp-dev").is_none() {
        return (404, "Not Found");
    }
    let Ok(body) = std::str::from_utf8(cx.body()) else {
        return (400, "body is not UTF-8");
    };
    let (file, line) = body.split_once('\n').unwrap_or((body, "1"));
    let line: u32 = line.trim().parse().unwrap_or(1).max(1);
    let file = file.trim();
    if !crate::http::stays_inside(file) {
        return (400, "not a file of the project");
    }
    let path = Path::new(root).join(file);
    if !path.is_file() {
        return (404, "no such file");
    }
    match open_in_editor(&path, line) {
        true => (200, "opened"),
        false => (500, "no editor found: set WISP_EDITOR, such as `code`"),
    }
}

/// Opens `path` at `line`: in `$WISP_EDITOR` or `$EDITOR` (one that draws
/// a window: a terminal one has no terminal here), else VS Code's `code
/// -g`, else whatever the system opens the file with.
#[cfg(debug_assertions)]
fn open_in_editor(path: &Path, line: u32) -> bool {
    use std::process::{Command, Stdio};
    let spawned = |mut c: Command| {
        c.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        match c.spawn() {
            // Waited for off the request, so it leaves no zombie behind.
            Ok(mut child) => {
                let _ = std::thread::Builder::new().spawn(move || child.wait());
                true
            }
            Err(_) => false,
        }
    };
    let at = format!("{}:{line}", path.display());
    let editor = std::env::var("WISP_EDITOR")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_default();
    let mut words = editor.split_whitespace();
    if let Some(program) = words.next() {
        let name = Path::new(program)
            .file_stem()
            .map(|s| s.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        const TERMINAL: [&str; 11] = [
            "vi", "vim", "nvim", "nano", "emacs", "micro", "kak", "hx", "helix", "ed", "joe",
        ];
        if !TERMINAL.contains(&name.as_str()) {
            let mut c = Command::new(program);
            c.args(words);
            match name.as_str() {
                "code" | "code-insiders" | "codium" | "cursor" | "windsurf" => c.arg("-g").arg(&at),
                _ => c.arg(&at),
            };
            if spawned(c) {
                return true;
            }
        }
    }
    let code = if cfg!(windows) { "code.cmd" } else { "code" };
    let mut c = Command::new(code);
    c.arg("-g").arg(&at);
    if spawned(c) {
        return true;
    }
    let system = match () {
        _ if cfg!(windows) => "explorer",
        _ if cfg!(target_os = "macos") => "open",
        _ => "xdg-open",
    };
    let mut c = Command::new(system);
    c.arg(path);
    spawned(c)
}

fn line<'a>(rest: &mut &'a str) -> Result<&'a str, &'static str> {
    let (l, r) = rest.split_once('\n').ok_or("truncated request")?;
    *rest = r;
    Ok(l)
}

/// Body: `path\nshape-hex\ncount\n` then per chunk `byte-length\n<bytes>`.
/// Refused unless the shape matches the template's exactly: the compiled
/// one's, or the last swap's. `old-hex>new-hex` gives it a new shape:
/// `wisp dev` found that only browser code changed with it, which a
/// compile would not change the program for (see `wisp_build::hot`).
fn swap<A: App>(body: &[u8]) -> Result<(), &'static str> {
    let hex = |s: &str| u64::from_str_radix(s, 16).map_err(|_| "bad shape");
    let mut rest = std::str::from_utf8(body).map_err(|_| "body is not UTF-8")?;
    let path = line(&mut rest)?;
    let shapes = line(&mut rest)?;
    let (from, to) = match shapes.split_once('>') {
        Some((a, b)) => (hex(a)?, hex(b)?),
        None => (hex(shapes)?, hex(shapes)?),
    };
    let count: usize = line(&mut rest)?.parse().map_err(|_| "bad count")?;
    let id = A::TEMPLATES
        .iter()
        .position(|(p, _)| *p == path)
        .ok_or("unknown template")?;
    // Each chunk takes a line at least: a count past that is a lie, and
    // reserving it could abort the process.
    let mut chunks = Vec::with_capacity(count.min(rest.len()));
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
    let at = swapped.iter().position(|s| s.0 == id);
    if at.map_or(A::TEMPLATES[id].1, |k| swapped[k].1) != from {
        return Err("template shape changed; rebuild needed");
    }
    match at {
        Some(k) => swapped[k] = (id, to, chunks),
        None => swapped.push((id, to, chunks)),
    }
    SWAPPED_ANY.store(true, Ordering::Release);
    Ok(())
}

/// Body: `path\nurl\nruntime-version\n` then the module's source. Only a
/// module the app serves can be swapped, made by the Wisp the app was (the
/// runtime it imports is the app's), and `url` must be `path?v=…`. A page
/// then names it by `url`, and `path` serves the source.
fn swap_module<A: App>(body: &[u8]) -> Result<(), &'static str> {
    let mut rest = std::str::from_utf8(body).map_err(|_| "body is not UTF-8")?;
    let path = line(&mut rest)?;
    let url = line(&mut rest)?;
    if line(&mut rest)? != crate::live::RUNTIME_VERSION {
        return Err("made by another version of Wisp; rebuild needed");
    }
    if A::client_module(path).is_none() {
        return Err("unknown module");
    }
    // Pages write it inside JSON and an attribute as it is.
    let version = url.strip_prefix(path).and_then(|v| v.strip_prefix("?v="));
    if !version.is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_alphanumeric())) {
        return Err("bad url");
    }
    let url: &'static str = Box::leak(url.to_owned().into_boxed_str());
    let source: &'static str = Box::leak(rest.to_owned().into_boxed_str());
    let mut modules = MODULES.write().unwrap_or_else(|e| e.into_inner());
    match modules.iter_mut().find(|m| m.0 == path) {
        Some(m) => (m.1, m.2) = (url, source),
        None => modules.push((path.to_owned(), url, source)),
    }
    MODULES_ANY.store(true, Ordering::Release);
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
/// built CSS (or `src/app.css`) and then the scoped styles, anything else
/// comes from `static/`.
pub(crate) fn read_file(root: &str, path: &str) -> Option<(Vec<u8>, String)> {
    let root = Path::new(root);
    if path == crate::protocol::APP_CSS_PATH {
        return app_css(root).map(|css| (css, "css".into()));
    }
    let file = (root.join("static")).join(crate::http::safe_relative_path(path)?);
    let bytes = std::fs::read(&file).ok()?; // also fails for directories
    let ext = file
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    Some((bytes, ext))
}

/// The built CSS (or `src/app.css`), then the scoped styles; `None`
/// without either.
fn app_css(root: &Path) -> Option<Vec<u8>> {
    let built = root.join(".wisp").join("app.css");
    let app = std::fs::read(if built.is_file() {
        built
    } else {
        root.join("src").join("app.css")
    });
    let scoped = std::fs::read(root.join(crate::protocol::SCOPED_CSS));
    if app.is_err() && scoped.is_err() {
        return None;
    }
    let mut css = app.unwrap_or_default();
    if let Some(s) = scoped.ok().filter(|s| !s.is_empty()) {
        if !css.is_empty() {
            css.push(b'\n');
        }
        css.extend_from_slice(&s);
    }
    Some(css)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fuzz::{Fuzz, Rng, TEMPLATE, mutate};

    /// Swap bodies from a buggy or hostile local client: an answer, never a
    /// panic or an abort (a huge count once reserved that much memory).
    #[test]
    fn swap_takes_any_body() {
        let (path, shape) = TEMPLATE;
        let huge = format!("{path}\n{shape:x}\n{}\n", usize::MAX);
        assert_eq!(swap::<Fuzz>(huge.as_bytes()), Err("truncated request"));
        let peer: SocketAddr = "127.0.0.1:9".parse().unwrap();
        let stranger: SocketAddr = "192.0.2.1:9".parse().unwrap();
        let body = format!("{path}\n{shape:x}\n0\n");
        let swapped = endpoint::<Fuzz>(Method::Post, "/_wisp/dev/swap", body.as_bytes(), peer);
        let expect = if cfg!(debug_assertions) { 200 } else { 404 };
        assert_eq!(swapped.0, expect, "debug builds only");
        assert_eq!(
            endpoint::<Fuzz>(Method::Post, "/_wisp/dev/swap", body.as_bytes(), stranger).0,
            404
        );
        // A new shape, given with the one it replaces; then only that one
        // is taken.
        let to = format!("{path}\n{shape:x}>def\n0\n");
        assert_eq!(swap::<Fuzz>(to.as_bytes()), Ok(()));
        assert!(
            swap::<Fuzz>(body.as_bytes()).is_err(),
            "the old shape is gone"
        );
        let back = format!("{path}\ndef>{shape:x}\n0\n");
        assert_eq!(swap::<Fuzz>(back.as_bytes()), Ok(()));
        let good = format!("{path}\n{shape:x}\n2\n5\nhello3\nabc").into_bytes();
        let mut rng = Rng::new(17);
        for _ in 0..20_000 {
            let mut b = good.clone();
            mutate(&mut rng, &mut b);
            let _ = swap::<Fuzz>(&b);
        }
    }

    /// A module is swapped in only where the app serves one, under a
    /// `?v=` URL that pages can write as it is; then pages name it by that.
    #[test]
    fn swaps_modules_the_app_serves() {
        use crate::fuzz::MODULE;
        assert_eq!(url(&MODULE), MODULE.url);
        let v = crate::live::RUNTIME_VERSION;
        let refused = [
            format!("/_app/c/t9.js\n/_app/c/t9.js?v=2\n{v}\nx"),
            format!("/_app/c/fuzz.js\n/_app/c/fuzz.js\n{v}\nx"),
            format!("/_app/c/fuzz.js\n/_app/c/fuzz.js?v=\"><script>\n{v}\nx"),
            format!("/_app/c/fuzz.js\n/other.js?v=2\n{v}\nx"),
            "/_app/c/fuzz.js\n/_app/c/fuzz.js?v=2\n0.0.1-other\nx".into(),
            "/_app/c/fuzz.js".into(),
        ];
        for body in &refused {
            assert!(swap_module::<Fuzz>(body.as_bytes()).is_err(), "{body}");
        }
        let body = format!("/_app/c/fuzz.js\n/_app/c/fuzz.js?v=2\n{v}\ndefine(\"fuzz\", 2);");
        assert_eq!(swap_module::<Fuzz>(body.as_bytes()), Ok(()));
        #[cfg(debug_assertions)]
        {
            assert_eq!(url(&MODULE), "/_app/c/fuzz.js?v=2");
            assert_eq!(module(MODULE.path), Some("define(\"fuzz\", 2);"));
        }
        let mut rng = Rng::new(5);
        for _ in 0..5_000 {
            let mut b = body.as_bytes().to_vec();
            mutate(&mut rng, &mut b);
            let _ = swap_module::<Fuzz>(&b);
        }
    }
}
