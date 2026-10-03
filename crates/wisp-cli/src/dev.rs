//! `wisp dev`: build, run, watch.
//!
//! One thread polls file mtimes (no watcher dependency, identical on every
//! OS). What changed decides what happens:
//!
//! - `.wisp` / `app.html` edits: hot swap into the running app, no compile,
//!   when only text or browser code changed (`hot_swap`): browsers morph in
//!   that file's part of the page and swap its module in place, keeping
//!   state. Anything else falls through to a rebuild.
//! - CSS output: tell browsers to swap the stylesheet (a CSS tool's input
//!   is its watcher's; a new `src/app.scss` or `postcss.config.*` changes
//!   the watchers).
//! - package.json and .env: rebuild, for the npm packages' versions and
//!   browser code's `env.PUBLIC_*`.
//! - `static/`: tell browsers to reload.
//! - anything else (Rust, Cargo.toml, new/removed routes): rebuild, restart,
//!   and let browsers morph to the new page.

use crate::cargo;
use crate::css;
use crate::events::Events;
use crate::term;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime};

const POLL: Duration = Duration::from_millis(50);
/// Editors often write a file in several steps; wait until it stops changing,
/// but no longer than `SETTLE_MAX`, or a file written without pause (a log)
/// would hold up every other change.
const SETTLE: Duration = Duration::from_millis(20);
const SETTLE_MAX: Duration = Duration::from_secs(1);

pub fn run(root: &Path, port: u16) -> Result<(), String> {
    let events =
        Events::start(port).map_err(|e| format!("Could not start the reload server: {e}."))?;
    let mut style = css::detect(root);
    let mut watchers = css::watch(root, &style)?;
    let mut app = Server {
        root: root.to_path_buf(),
        port,
        events_port: events.port,
        child: None,
        addr: None,
        shown: expected(port),
        slot: 0,
    };

    // The address the app will say it listens on, when it can be known
    // before then; if it turns out to be another, "Ready" says so.
    let url = app
        .shown
        .map(|a| format!("{}  ", term::bold(&format!("http://{a}"))));
    println!(
        "\n{}  {}{}\n",
        term::bold(&term::accent("Wisp")),
        url.unwrap_or_default(),
        term::dim("Ctrl+C to stop")
    );
    let mut files = scan(root);
    term::step("Building");
    // The app as the running build has it, for what can be swapped in.
    let mut base = rebuild(&mut app, &events, root, true, "");
    let mut had_styles = wisp_build::write_styles(root).is_ok_and(|(_, any)| any);

    loop {
        sleep(POLL);
        let mut now = scan(root);
        if now == files {
            continue;
        }
        let settling = Instant::now();
        while settling.elapsed() < SETTLE_MAX {
            sleep(SETTLE);
            let again = scan(root);
            if again == now {
                break;
            }
            now = again;
        }
        let changed = diff(&files, &now);
        files = now;
        // What builds the CSS changed: new watchers, which build it first.
        if changed.iter().any(|(rel, _)| css::decides(rel)) {
            let now = css::detect(root);
            if now != style {
                drop(std::mem::take(&mut watchers));
                style = now;
                match css::watch(root, &style) {
                    Ok(w) => watchers = w,
                    Err(e) => term::failed(&e),
                }
            }
        }
        let started = Instant::now();
        let names: Vec<_> = changed.iter().map(|(p, _)| p.as_str()).collect();
        let names = names.join(", ");

        let mut rebuild_needed = false;
        let mut templates = Vec::new();
        let (mut css, mut full) = (false, false);
        for (rel, kind) in &changed {
            if rel.ends_with(".wisp") || rel == "src/app.html" {
                // A stories file is a template per story: only a build splits it.
                if *kind == Change::Modified && !rel.ends_with(".stories.wisp") {
                    templates.push(rel.as_str())
                } else {
                    rebuild_needed = true
                }
            } else if rel == ".wisp/app.css" || (rel == "src/app.css" && style.plain()) {
                css = true;
            } else if style.owns(rel) || css::POSTCSS_CONFIGS.contains(&rel.as_str()) {
                // A CSS watcher reads it and will write .wisp/app.css.
            } else if rel.starts_with("static/") {
                full = true;
            } else {
                rebuild_needed = true;
            }
        }

        if !rebuild_needed && !templates.is_empty() {
            // Scoped styles: a stylesheet swap, or a build when the app
            // gains its first or loses its last (its pages' <link>).
            match wisp_build::write_styles(root) {
                Ok((changed, any)) => {
                    rebuild_needed = any != had_styles;
                    had_styles = any;
                    css |= changed;
                }
                Err(e) => term::failed(&e),
            }
        }
        // Why browsers swap their modules whole after the rebuild, if so.
        let mut why = String::new();
        if !rebuild_needed && !templates.is_empty() {
            match hot_swap(&app, &mut base, &templates) {
                Swap::Done(news, warnings) => {
                    if !news.is_empty() {
                        events.send("hot", &news);
                    }
                    term::changed(
                        &templates.join(", "),
                        &format!("swapped in {}ms", started.elapsed().as_millis()),
                    );
                    for w in &warnings {
                        term::warn(w);
                    }
                }
                Swap::Invalid(e) => {
                    term::changed(&templates.join(", "), "");
                    term::failed(&e);
                    show_error(
                        &events,
                        "Build Failed",
                        &summary(place(&e), 1),
                        "Template Check",
                        &e,
                    );
                }
                Swap::NeedsRebuild(reason) => {
                    rebuild_needed = true;
                    why = reason;
                }
            }
        }
        if rebuild_needed {
            term::changed(&names, "");
            if let Some(next) = rebuild(&mut app, &events, root, false, &why) {
                base = Some(next);
            }
        } else if full {
            events.send("full", "");
            term::changed(&names, "reloaded");
        } else if css {
            events.send("css", "");
            term::changed(&names, "styles swapped");
        }
    }
}

/// Builds and restarts the app; then the app as that build has it, for
/// what can be swapped in later. `first` shows cargo's progress, since the
/// first build can take a minute; later ones only say how they went. `why`
/// is told to browsers: why their modules are swapped whole, if they are.
fn rebuild(
    app: &mut Server,
    events: &Events,
    root: &Path,
    first: bool,
    why: &str,
) -> Option<wisp_build::Hot> {
    let started = Instant::now();
    events.send("building", "");
    // Route and template errors are found in milliseconds without cargo.
    // Taken before cargo reads the files: a save meanwhile is a change to it.
    let hot = match wisp_build::hot(root) {
        Ok(hot) => hot,
        Err(e) => {
            term::failed(&e);
            show_error(
                events,
                "Build Failed",
                &summary(place(&e), 1),
                "Wisp Check",
                &e,
            );
            return None;
        }
    };
    for w in &hot.warnings {
        term::warn(w);
    }
    let build = cargo::build(root, false, !first);
    let Some(exe) = build.exe.filter(|_| build.ok) else {
        let errors = if build.count == 1 {
            "1 error".to_string()
        } else {
            format!("{} errors", build.count)
        };
        term::failed(&format!("Build failed with {errors}."));
        show_error(
            events,
            "Build Failed",
            &summary(build.first, build.count),
            "Compiler Output",
            &build.errors,
        );
        return None;
    };
    match app.restart(&exe) {
        Ok(()) => {
            let took = term::dim(&format!("in {:.1}s", started.elapsed().as_secs_f64()));
            let what = if first { "Ready" } else { "Rebuilt" };
            match app.addr {
                Some(a) if app.shown != app.addr => {
                    app.shown = app.addr;
                    term::done(&format!(
                        "{what} at {} {took}",
                        term::bold(&format!("http://{a}"))
                    ));
                }
                _ => term::done(&format!("{what} {took}")),
            }
            if let Some(a) = app.addr {
                events.allow(a);
            }
            events.send("reload", why);
            Some(hot)
        }
        Err(e) => {
            term::failed(&e);
            let summary = e.lines().next().unwrap_or_default();
            show_error(events, "App Didn't Start", summary, "Startup", &e);
            None
        }
    }
}

/// Opens the error dialog in every tab: its title, one sentence, the code
/// strip's label, then the text itself.
fn show_error(events: &Events, title: &str, summary: &str, label: &str, text: &str) {
    events.send("title", &format!("{title}\n{summary}\n{label}"));
    events.send("error", text);
}

/// Where an error from `wisp check` is: `src/routes/+page.wisp:4:1: ...`.
fn place(e: &str) -> Option<(String, usize)> {
    let (at, _) = e.split_once(": ")?;
    let mut parts = at.split(':');
    let file = parts.next()?;
    let line = parts.next()?.parse().ok()?;
    Some((file.to_string(), line))
}

/// The dialog's sentence: where to look, and what to do.
fn summary(first: Option<(String, usize)>, count: usize) -> String {
    let fix = "Save a fix and the page updates.";
    match first {
        Some((file, line)) if count > 1 => {
            format!("{count} errors. The first is in {file}, line {line}. {fix}")
        }
        Some((file, line)) => format!("{file}, line {line}. {fix}"),
        None => fix.to_string(),
    }
}

enum Swap {
    /// Swapped in: what browsers are told (the `hot` event: `m URL` per
    /// module to load again, then `r file` per template whose text to
    /// morph in), and the accessibility warnings of what changed.
    Done(String, Vec<String>),
    /// The template does not parse; show the error, keep the running app.
    Invalid(String),
    /// Only a compile can apply it; why browsers should then swap their
    /// modules whole, if they should.
    NeedsRebuild(String),
}

/// Swaps changed templates into the running app. Text alone: their static
/// text, as the shape the app has is still theirs. Else, when a compile
/// would make the same program but for templates' text and shapes and the
/// browser modules (`wisp_build::hot`), those. Anything else is a compile.
fn hot_swap(app: &Server, base: &mut Option<wisp_build::Hot>, templates: &[&str]) -> Swap {
    let Some(addr) = app.addr else {
        return Swap::NeedsRebuild(String::new());
    };
    let mut news = String::new();
    let mut all = Vec::new();
    let mut whole = true;
    for rel in templates {
        let (chunks, shape, warnings) = match wisp_build::hot_chunks(&app.root, rel) {
            Ok(x) => x,
            Err(e) => return Swap::Invalid(e),
        };
        if !post_chunks(addr, rel, shape, shape, &chunks) {
            whole = false;
            break;
        }
        all.extend(warnings);
        // Only its style changed: the stylesheet swap is all.
        let kept = base
            .as_mut()
            .and_then(|b| b.templates.iter_mut().find(|t| t.rel == *rel));
        match kept {
            Some(t) if t.chunks == chunks => {}
            Some(t) => {
                t.chunks = chunks;
                news.push_str(&format!("r {rel}\n"));
            }
            None => news.push_str(&format!("r {rel}\n")),
        }
    }
    if whole {
        return Swap::Done(news, all);
    }
    // Browser code changed too.
    let next = match wisp_build::hot(&app.root) {
        Ok(next) => next,
        Err(e) => return Swap::Invalid(e),
    };
    let Some(old) = base.as_ref() else {
        return Swap::NeedsRebuild(String::new());
    };
    if next.rust != old.rust {
        return Swap::NeedsRebuild(why(old, &next, templates));
    }
    // The modules first: the app takes them only from the Wisp it was
    // built with, whose compile this one stands for.
    let (mut modules, mut regions) = (String::new(), String::new());
    for (path, url, source) in &next.files {
        let Some(o) = old.files.iter().find(|o| o.0 == *path) else {
            return Swap::NeedsRebuild(String::new());
        };
        if o.2 == *source {
            continue;
        }
        let version = wisp_build::runtime_version();
        let mut body = format!("{path}\n{url}\n{version}\n").into_bytes();
        body.extend_from_slice(source.as_bytes());
        if request(addr, "POST", "/_wisp/dev/module", &body) != Some(200) {
            return Swap::NeedsRebuild(String::new());
        }
        if path.ends_with(".js") {
            modules.push_str(&format!("m {url}\n"));
        }
    }
    if modules.is_empty() {
        return Swap::NeedsRebuild(String::new());
    }
    for t in &next.templates {
        let Some(o) = old.templates.iter().find(|o| o.rel == t.rel) else {
            return Swap::NeedsRebuild(String::new());
        };
        if (o.shape != t.shape || o.chunks != t.chunks)
            && !post_chunks(addr, &t.rel, o.shape, t.shape, &t.chunks)
        {
            return Swap::NeedsRebuild(String::new());
        }
        if o.chunks != t.chunks {
            regions.push_str(&format!("r {}\n", t.rel));
        }
    }
    let warnings = (next.warnings.iter())
        .filter(|w| templates.iter().any(|t| w.starts_with(&format!("{t}:"))))
        .cloned()
        .collect();
    *base = Some(next);
    modules.push_str(&regions);
    Swap::Done(modules, warnings)
}

/// Sends a template's static text to the app, for the template whose shape
/// is `from` there, which then has shape `to`.
fn post_chunks(addr: SocketAddr, rel: &str, from: u64, to: u64, chunks: &[String]) -> bool {
    let shapes = match from == to {
        true => format!("{from:016x}"),
        false => format!("{from:016x}>{to:016x}"),
    };
    let mut body = format!("{rel}\n{shapes}\n{}\n", chunks.len()).into_bytes();
    for c in chunks {
        body.extend_from_slice(format!("{}\n", c.len()).as_bytes());
        body.extend_from_slice(c.as_bytes());
    }
    request(addr, "POST", "/_wisp/dev/swap", &body) == Some(200)
}

/// Why browsers swap a changed template's module whole once it is
/// compiled: its `{@props}` or its `---` block changed. Empty for another
/// change, after which they keep its state.
fn why(old: &wisp_build::Hot, next: &wisp_build::Hot, templates: &[&str]) -> String {
    for rel in templates {
        let find = |h: &wisp_build::Hot| {
            h.templates
                .iter()
                .find(|t| t.rel == *rel)
                .map(|t| (t.props, t.block))
        };
        match (find(old), find(next)) {
            (Some(o), Some(n)) if o.0 != n.0 => return format!("{rel}: its props changed"),
            (Some(o), Some(n)) if o.1 != n.1 => return format!("{rel}: its --- block changed"),
            _ => {}
        }
    }
    String::new()
}

/// The app process. Runs from a copy of the executable so `cargo build` can
/// replace the original while the old server keeps answering.
struct Server {
    root: PathBuf,
    port: u16,
    events_port: u16,
    child: Option<Child>,
    /// Where the running app is reached, from what it printed.
    addr: Option<SocketAddr>,
    /// The address the terminal last gave.
    shown: Option<SocketAddr>,
    slot: u8,
}

impl Server {
    fn restart(&mut self, exe: &Path) -> Result<(), String> {
        let dir = self.root.join(".wisp").join("run");
        crate::make_dir(&dir)?;
        self.slot ^= 1;
        let copy = dir.join(format!("app-{}{}", self.slot, std::env::consts::EXE_SUFFIX));
        fs::copy(exe, &copy).map_err(|e| format!("Could not copy {}: {e}.", exe.display()))?;
        self.stop();
        let mut cmd = Command::new(&copy);
        cmd.current_dir(&self.root)
            .env("PORT", self.port.to_string())
            .env("WISP_DEV_EVENTS", self.events_port.to_string())
            // The app exits when its stdin closes. The Child keeps the other
            // end until the app is stopped, and when `wisp dev` is killed
            // without the chance to stop it, the system closes it, so the
            // app never outlives the CLI holding on to the port.
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        if std::env::var_os("HOST").is_none() {
            cmd.env("HOST", "127.0.0.1");
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Could not start {}: {e}.", copy.display()))?;

        // The app prints a line once it is listening. Waiting for that line
        // instead of polling the port matters on Windows, where connecting
        // to a port nobody listens on takes 2s to fail. The pipe closing
        // first means the app died during startup (port in use...).
        let stdout = child.stdout.take().expect("stdout is piped");
        self.child = Some(child);
        let (ready, listening) = mpsc::channel();
        std::thread::spawn(move || {
            let mut ready = Some(ready);
            term::each_line(stdout, |line| {
                match line.strip_prefix("wisp: listening on http://") {
                    // `wisp dev` says where the app is itself.
                    Some(addr) => {
                        if let Some(r) = ready.take() {
                            let _ = r.send(addr.trim().parse::<SocketAddr>().ok());
                        }
                    }
                    None => println!("{line}"),
                }
            });
        });
        match listening.recv_timeout(Duration::from_secs(10)) {
            Ok(addr) => {
                self.addr = Some(reachable(
                    addr.unwrap_or(SocketAddr::from(([127, 0, 0, 1], self.port))),
                ));
                Ok(())
            }
            Err(RecvTimeoutError::Timeout) => Err(
                "The app did not start listening within 10s.\nIts output is in the terminal."
                    .into(),
            ),
            Err(RecvTimeoutError::Disconnected) => {
                // The pipe closes when the app exits, or when it closes its
                // stdout and goes on running; never wait on the second.
                let mut child = self.child.take().expect("started above");
                let deadline = Instant::now() + Duration::from_secs(1);
                let status = loop {
                    match child.try_wait() {
                        Ok(Some(status)) => break Some(status),
                        Ok(None) if Instant::now() < deadline => sleep(Duration::from_millis(10)),
                        _ => {
                            let _ = child.kill();
                            let _ = child.wait();
                            break None;
                        }
                    }
                };
                match status {
                    Some(s) => Err(format!("The app stopped while starting ({s}).\nIts output is in the terminal; a port already in use is the usual cause.")),
                    None => Err("The app closed its output before it was listening, so it was stopped.\nwisp dev reads that output to know when the app is ready.".into()),
                }
            }
        }
    }

    fn stop(&mut self) {
        self.addr = None;
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Where the app will listen, when the CLI can tell before it starts: on
/// `$HOST` if set, else on loopback, at the port asked for (0 is any port).
fn expected(port: u16) -> Option<SocketAddr> {
    let ip = match std::env::var("HOST") {
        Ok(host) => host.parse().ok()?,
        Err(_) => IpAddr::V4(Ipv4Addr::LOCALHOST),
    };
    (port != 0).then(|| reachable(SocketAddr::new(ip, port)))
}

/// The address to reach an app at from this machine. One listening on every
/// address (`0.0.0.0`, `::`) is reached on loopback.
fn reachable(addr: SocketAddr) -> SocketAddr {
    match addr.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => {
            SocketAddr::new(Ipv4Addr::LOCALHOST.into(), addr.port())
        }
        IpAddr::V6(ip) if ip.is_unspecified() => {
            SocketAddr::new(Ipv6Addr::LOCALHOST.into(), addr.port())
        }
        _ => addr,
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A minimal HTTP/1.1 request to the app; returns the status. The connect
/// timeout keeps a dead app from costing Windows' 2s refusal.
fn request(addr: SocketAddr, method: &str, path: &str, body: &[u8]) -> Option<u16> {
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(250)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let head = format!(
        "{method} {path} HTTP/1.1\r\nhost: {addr}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes()).ok()?;
    s.write_all(body).ok()?;
    let mut resp = Vec::new();
    s.read_to_end(&mut resp).ok()?;
    let line = resp.split(|&b| b == b'\r').next()?;
    std::str::from_utf8(line.get(9..12)?).ok()?.parse().ok()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Change {
    Added,
    Modified,
    Removed,
}

/// A file's mtime, size, and, for a small one saved in the last seconds, a
/// hash of its text (0 otherwise).
type Snapshot = HashMap<String, (SystemTime, u64, u64)>;

/// A save of the same size inside the file system's clock tick changes
/// neither mtime nor size; so a small file that was just saved is hashed.
fn stamp(path: &Path, m: SystemTime, len: u64) -> (SystemTime, u64, u64) {
    let fresh = len < 1 << 20 && m.elapsed().is_ok_and(|age| age.as_secs() < 3);
    let hash = match fresh.then(|| fs::read(path)) {
        Some(Ok(text)) => wisp_build::fnv1a(&text) | 1,
        _ => 0,
    };
    (m, len, hash)
}

/// Every watched file (relative, `/`-separated) with its `stamp`.
fn scan(root: &Path) -> Snapshot {
    fn walk(root: &Path, dir: &Path, out: &mut Snapshot) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let path = e.path();
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() {
                walk(root, &path, out);
            } else if scratch(&e.file_name().to_string_lossy()) {
                continue;
            } else if let Ok(m) = meta.modified() {
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(rel, stamp(&path, m, meta.len()));
            }
        }
    }
    let mut out = Snapshot::new();
    walk(root, &root.join("src"), &mut out);
    walk(root, &root.join("static"), &mut out);
    let top = [
        "Cargo.toml",
        "build.rs",
        ".wisp/app.css",
        "package.json",
        ".env",
    ];
    for f in top.iter().chain(&css::POSTCSS_CONFIGS) {
        if let Ok(meta) = fs::metadata(root.join(f))
            && let Ok(m) = meta.modified()
        {
            out.insert(f.to_string(), stamp(&root.join(f), m, meta.len()));
        }
    }
    out
}

/// An editor's swap, backup or half-saved file: never part of the app, and
/// changed on every save, so watching it would rebuild for nothing.
fn scratch(name: &str) -> bool {
    name.starts_with(['.', '#'])
        || name.ends_with('~')
        || name.ends_with(".swp")
        || name.ends_with(".swx")
        || name.ends_with(".tmp")
        || name.contains("___jb_") // JetBrains' safe write
        || name == "4913" // Vim's test that it may write the folder
}

fn diff(old: &Snapshot, new: &Snapshot) -> Vec<(String, Change)> {
    let mut out: Vec<(String, Change)> = new
        .iter()
        .filter_map(|(p, t)| match old.get(p) {
            None => Some((p.clone(), Change::Added)),
            Some(o) if o.0 != t.0 || o.1 != t.1 || (o.2 != t.2 && o.2 != 0 && t.2 != 0) => {
                Some((p.clone(), Change::Modified))
            }
            Some(_) => None,
        })
        .collect();
    out.extend(
        old.keys()
            .filter(|p| !new.contains_key(*p))
            .map(|p| (p.clone(), Change::Removed)),
    );
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_editor_files() {
        for name in [
            ".+page.wisp.swp",
            ".page.swx",
            "#+page.wisp#",
            "+page.wisp~",
            "app.css.tmp",
            "+page.rs___jb_tmp___",
            "4913",
        ] {
            assert!(scratch(name), "{name}");
        }
        for name in ["+page.wisp", "+page.rs", "app.css", "words.txt", "4913.txt"] {
            assert!(!scratch(name), "{name}");
        }
    }

    #[test]
    fn reaches_every_address_on_loopback() {
        let at = |s: &str| reachable(s.parse().unwrap()).to_string();
        assert_eq!(at("0.0.0.0:3000"), "127.0.0.1:3000");
        assert_eq!(at("[::]:3000"), "[::1]:3000");
        assert_eq!(at("[::1]:3001"), "[::1]:3001");
        assert_eq!(at("192.168.1.5:80"), "192.168.1.5:80");
    }
}
