//! `wisp dev`: build, run, watch.
//!
//! One thread polls file mtimes (no watcher dependency, identical on every
//! OS). What changed decides what happens:
//!
//! - `.wisp` / `app.html` text edits: hot swap into the running app, no
//!   compile. If the template's shape changed, fall through to a rebuild.
//! - CSS output: tell browsers to swap the stylesheet.
//! - `static/`: tell browsers to reload.
//! - anything else (Rust, Cargo.toml, new/removed routes): rebuild, restart,
//!   and let browsers morph to the new page.

use crate::cargo;
use crate::css;
use crate::events::Events;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime};

const POLL: Duration = Duration::from_millis(50);
/// Editors often write a file in several steps; wait until it stops changing.
const SETTLE: Duration = Duration::from_millis(20);

pub fn run(root: &Path, port: u16) -> Result<(), String> {
    let events = Events::start().map_err(|e| format!("event server: {e}"))?;
    let _tailwind = css::watch(root)?;
    let mut app = Server { root: root.to_path_buf(), port, events_port: events.port, child: None, slot: 0 };

    println!("wisp dev: http://127.0.0.1:{port}");
    let mut files = scan(root);
    rebuild(&mut app, &events, root);

    loop {
        sleep(POLL);
        let mut now = scan(root);
        if now == files {
            continue;
        }
        loop {
            sleep(SETTLE);
            let again = scan(root);
            if again == now {
                break;
            }
            now = again;
        }
        let changed = diff(&files, &now);
        files = now;
        let started = Instant::now();

        let mut rebuild_needed = false;
        let mut templates = Vec::new();
        let (mut css, mut full) = (false, false);
        for (rel, kind) in &changed {
            if rel.ends_with(".wisp") || rel == "src/app.html" {
                if *kind == Change::Modified { templates.push(rel.as_str()) } else { rebuild_needed = true }
            } else if rel == ".wisp/app.css" || (rel == "src/app.css" && matches!(css::detect(root), css::Css::Plain)) {
                css = true;
            } else if rel == "src/app.css" {
                // Tailwind is watching it and will write .wisp/app.css.
            } else if rel.starts_with("static/") {
                full = true;
            } else {
                rebuild_needed = true;
            }
        }

        if !rebuild_needed && !templates.is_empty() {
            match hot_swap(&app, &templates) {
                Swap::Done => {
                    events.send("reload", "");
                    println!("  ~ {} hot-swapped in {}ms", templates.join(", "), started.elapsed().as_millis());
                }
                Swap::Invalid(e) => {
                    eprintln!("\n{e}\n");
                    events.send("error", &e);
                }
                Swap::NeedsRebuild => rebuild_needed = true,
            }
        }
        if rebuild_needed {
            let names: Vec<_> = changed.iter().map(|(p, _)| p.as_str()).collect();
            println!("  ~ {}", names.join(", "));
            rebuild(&mut app, &events, root);
        } else if full {
            events.send("full", "");
        } else if css {
            events.send("css", "");
        }
    }
}

fn rebuild(app: &mut Server, events: &Events, root: &Path) {
    let started = Instant::now();
    // Route and template errors are found in milliseconds without cargo.
    if let Err(e) = wisp_build::check(root) {
        eprintln!("\nwisp: {e}\n");
        events.send("error", &format!("wisp: {e}"));
        return;
    }
    let build = cargo::build(root, false);
    let Some(exe) = build.exe.filter(|_| build.ok) else {
        events.send("error", &build.errors);
        return;
    };
    match app.restart(&exe) {
        Ok(()) => {
            println!("  built and restarted in {:.1}s", started.elapsed().as_secs_f64());
            events.send("reload", "");
        }
        Err(e) => {
            eprintln!("wisp: {e}");
            events.send("error", &e);
        }
    }
}

enum Swap {
    Done,
    /// The template does not parse; show the error, keep the running app.
    Invalid(String),
    /// New shape or unknown template: only a compile can apply it.
    NeedsRebuild,
}

fn hot_swap(app: &Server, templates: &[&str]) -> Swap {
    for rel in templates {
        let (chunks, shape) = match wisp_build::hot_chunks(&app.root, rel) {
            Ok(x) => x,
            Err(e) => return Swap::Invalid(e),
        };
        let mut body = format!("{rel}\n{shape:016x}\n{}\n", chunks.len()).into_bytes();
        for c in &chunks {
            body.extend_from_slice(format!("{}\n", c.len()).as_bytes());
            body.extend_from_slice(c.as_bytes());
        }
        match request(app.port, "POST", "/_wisp/dev/swap", &body) {
            Some(200) => {}
            _ => return Swap::NeedsRebuild,
        }
    }
    Swap::Done
}

/// The app process. Runs from a copy of the executable so `cargo build` can
/// replace the original while the old server keeps answering.
struct Server {
    root: PathBuf,
    port: u16,
    events_port: u16,
    child: Option<Child>,
    slot: u8,
}

impl Server {
    fn restart(&mut self, exe: &Path) -> Result<(), String> {
        let dir = self.root.join(".wisp").join("run");
        fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        self.slot ^= 1;
        let copy = dir.join(format!("app-{}{}", self.slot, std::env::consts::EXE_SUFFIX));
        fs::copy(exe, &copy).map_err(|e| format!("copying {}: {e}", exe.display()))?;
        self.stop();
        let mut child = Command::new(&copy)
            .current_dir(&self.root)
            .env("PORT", self.port.to_string())
            .env("WISP_DEV_EVENTS", self.events_port.to_string())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("starting {}: {e}", copy.display()))?;

        // The app prints a line once it is listening. Waiting for that line
        // instead of polling the port matters on Windows, where connecting
        // to a port nobody listens on takes 2s to fail. The pipe closing
        // first means the app died during startup (port in use...).
        let stdout = child.stdout.take().expect("stdout is piped");
        self.child = Some(child);
        let (ready, listening) = mpsc::channel();
        std::thread::spawn(move || {
            let mut ready = Some(ready);
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                println!("{line}");
                if line.starts_with("wisp: listening on ")
                    && let Some(r) = ready.take()
                {
                    let _ = r.send(());
                }
            }
        });
        match listening.recv_timeout(Duration::from_secs(10)) {
            Ok(()) => Ok(()),
            Err(RecvTimeoutError::Timeout) => Err("the app did not start listening within 10s".into()),
            Err(RecvTimeoutError::Disconnected) => {
                let status = self.child.take().and_then(|mut c| c.wait().ok());
                Err(format!("the app exited during startup ({})", status.map_or("unknown status".into(), |s| s.to_string())))
            }
        }
    }

    fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A minimal HTTP/1.1 request to the app on loopback; returns the status.
/// The connect timeout keeps a dead app from costing Windows' 2s refusal.
fn request(port: u16, method: &str, path: &str, body: &[u8]) -> Option<u16> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(250)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let head = format!("{method} {path} HTTP/1.1\r\nhost: 127.0.0.1\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len());
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

type Snapshot = HashMap<String, SystemTime>;

/// Every watched file (relative, `/`-separated) and its mtime.
fn scan(root: &Path) -> Snapshot {
    fn walk(root: &Path, dir: &Path, out: &mut Snapshot) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let path = e.path();
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() {
                walk(root, &path, out);
            } else if let Ok(m) = meta.modified() {
                let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                out.insert(rel, m);
            }
        }
    }
    let mut out = Snapshot::new();
    walk(root, &root.join("src"), &mut out);
    walk(root, &root.join("static"), &mut out);
    for f in ["Cargo.toml", "build.rs", ".wisp/app.css"] {
        if let Ok(m) = fs::metadata(root.join(f)).and_then(|m| m.modified()) {
            out.insert(f.to_string(), m);
        }
    }
    out
}

fn diff(old: &Snapshot, new: &Snapshot) -> Vec<(String, Change)> {
    let mut out: Vec<(String, Change)> = new
        .iter()
        .filter_map(|(p, t)| match old.get(p) {
            None => Some((p.clone(), Change::Added)),
            Some(o) if o != t => Some((p.clone(), Change::Modified)),
            Some(_) => None,
        })
        .collect();
    out.extend(old.keys().filter(|p| !new.contains_key(*p)).map(|p| (p.clone(), Change::Removed)));
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}
