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
//! - package.json and .env: rebuild and restart, for the npm packages'
//!   versions, browser code's `env.PUBLIC_*` and the server's `wisp::env`.
//! - `static/`: tell browsers to reload.
//! - a `.rs` file saved with the same code (a formatter, a comment): nothing.
//! - anything else (Rust, Cargo.toml, new/removed routes): rebuild, restart,
//!   and let browsers morph to the new page. Edits to the app's own `.rs`
//!   and `.wisp` files compile without cargo, `rustc` run as cargo ran it
//!   (`quick`).

use crate::cargo;
use crate::css;
use crate::events::Events;
use crate::quick::{self, Quick};
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

/// How many ports after the one asked for the app may try, the first start.
const PORT_TRIES: u16 = 20;
const POLL: Duration = Duration::from_millis(50);
/// Editors often write a file in several steps; wait until it stops changing,
/// but no longer than `SETTLE_MAX`, or a file written without pause (a log)
/// would hold up every other change.
const SETTLE: Duration = Duration::from_millis(25);
/// How long a restarted app has to answer a request before the build is
/// called failed, and before any browser is told to reload.
const HEALTH_WAIT: Duration = Duration::from_secs(10);
const SETTLE_MAX: Duration = Duration::from_secs(1);

pub fn run(root: &Path, port: u16) -> Result<(), String> {
    crate::fonts::build(root);
    let events =
        Events::start(port).map_err(|e| format!("Could not start the reload server: {e}."))?;
    let mut style = css::detect(root);
    let mut watchers = css::watch(root, &style)?;
    let mut app = Server {
        root: root.to_path_buf(),
        abs: std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf()),
        port,
        events_port: events.port,
        child: None,
        addr: None,
        shown: None,
        tries: PORT_TRIES,
        slot: 0,
    };

    // The port may move on, so "Ready" gives the address once it is known.
    println!(
        "
{}  {}
",
        term::bold(&term::accent("Wisp")),
        term::dim("Ctrl+C to stop")
    );
    let mut files = scan(root);
    term::step("Building");
    let mut fast = Fast {
        recipe: None,
        lld: true,
    };
    // The app as the running build has it, for what can be swapped in.
    let mut base = rebuild(&mut app, &events, root, true, "", &mut fast, false);
    // Whether the running app is the last save, and what each Rust file's
    // code is, to tell a save that changes none.
    let mut clean = base.is_some();
    let mut codes: HashMap<String, u64> = (files.keys())
        .filter(|rel| rel.ends_with(".rs"))
        .filter_map(|rel| Some((rel.clone(), code_hash(root, rel)?)))
        .collect();
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
        // A formatter or a comment: the same program, nothing to build.
        let mut same = clean && !changed.is_empty();
        for (rel, kind) in &changed {
            if !rel.ends_with(".rs") {
                same = false;
                continue;
            }
            let now = code_hash(root, rel);
            let was = match now {
                Some(h) => codes.insert(rel.clone(), h),
                None => codes.remove(rel),
            };
            same &= *kind == Change::Modified && now.is_some() && now == was;
        }
        let names: Vec<_> = changed.iter().map(|(p, _)| p.as_str()).collect();
        let names = names.join(", ");
        if same {
            term::changed(&names, "no code changed");
            continue;
        }
        let direct = own_sources(&changed);

        let Plan {
            rebuild: mut rebuild_needed,
            templates,
            mut css,
            full,
        } = plan(&changed, &style);

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
            let built = rebuild(&mut app, &events, root, false, &why, &mut fast, direct);
            clean = built.is_some();
            if let Some(next) = built {
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

/// What a set of changes asks for: a compile, templates to swap in, a
/// stylesheet swap, a full reload.
#[derive(Debug, Default, PartialEq)]
struct Plan<'a> {
    rebuild: bool,
    templates: Vec<&'a str>,
    css: bool,
    full: bool,
}

/// Decides what `changed` needs. A template added, removed or renamed (a
/// remove and an add) changes the routes: a compile. Only a modified one
/// can be swapped in.
fn plan<'a>(changed: &'a [(String, Change)], style: &css::Css) -> Plan<'a> {
    let mut p = Plan::default();
    for (rel, kind) in changed {
        if rel.ends_with(".wisp") || rel == "src/app.html" {
            if *kind == Change::Modified {
                p.templates.push(rel.as_str())
            } else {
                p.rebuild = true
            }
        } else if rel == ".wisp/app.css" || (rel == "src/app.css" && style.plain()) {
            p.css = true;
        } else if style.owns(rel) || css::POSTCSS_CONFIGS.contains(&rel.as_str()) {
            // A CSS watcher reads it and will write .wisp/app.css.
        } else if rel.starts_with("static/") {
            p.full = true;
        } else {
            p.rebuild = true;
        }
    }
    p
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
    fast: &mut Fast,
    direct: bool,
) -> Option<wisp_build::Hot> {
    let started = Instant::now();
    events.send("building", "");
    // Route and template errors are found in milliseconds without cargo.
    // Taken before cargo reads the files: a save meanwhile is a change to it.
    // Absolute: the code names the app's files, and is compiled from
    // another folder when cargo does not.
    let mut hot = match wisp_build::hot(&app.abs) {
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
    // The generated code is only for the compile; what is kept is without.
    let code = std::mem::take(&mut hot.code);
    let direct = fast
        .recipe
        .as_ref()
        .filter(|_| direct && quick::possible(root));
    let build = match direct {
        Some(recipe) => match quick::build(root, recipe, &code, &mut fast.lld) {
            Quick::Done(b) => b,
            Quick::Cargo => cargo_build(root, !first, fast),
        },
        None => cargo_build(root, !first, fast),
    };
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
            // The app says it is listening a moment before it answers; a
            // browser told sooner would fetch from a server not there yet.
            if let Some(a) = app.addr
                && !healthy(a, HEALTH_WAIT)
            {
                let e = "The app is listening but does not answer requests.\nIts output is in the terminal.";
                term::failed(e);
                show_error(
                    events,
                    "App Didn't Start",
                    e.lines().next().unwrap_or_default(),
                    "Startup",
                    e,
                );
                return None;
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

/// Whether only the app's own Rust and templates changed: a build of them
/// needs nothing of cargo's (see quick.rs).
fn own_sources(changed: &[(String, Change)]) -> bool {
    (changed.iter()).all(|(rel, _)| {
        rel.strip_prefix("src/")
            .is_some_and(|r| r.ends_with(".rs") || r.ends_with(".wisp"))
    })
}

/// What rebuilds without cargo keep (see quick.rs).
struct Fast {
    /// How cargo last ran rustc for the app.
    recipe: Option<quick::Recipe>,
    /// Whether to link with `rust-lld`.
    lld: bool,
}

/// A cargo build, which also records how it ran rustc (for the app that
/// can be built without it).
fn cargo_build(root: &Path, quiet: bool, fast: &mut Fast) -> cargo::Build {
    fast.recipe = None;
    if !quick::possible(root) || !direct_on() {
        return cargo::build(root, false, quiet);
    }
    let b = quick::record(root, quiet);
    fast.recipe = quick::Recipe::load(root);
    b
}

/// Whether saves may build with rustc directly: the default on Windows,
/// where it is measured and tested; elsewhere only with `WISP_DEV_DIRECT=1`
/// (experimental). `WISP_DEV_CARGO` turns it off everywhere.
fn direct_on() -> bool {
    let set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
    !set("WISP_DEV_CARGO")
        && (cfg!(windows) || std::env::var_os("WISP_DEV_DIRECT").is_some_and(|v| v == "1"))
}

/// A Rust file's code, as `quick::code_hash` has it.
fn code_hash(root: &Path, rel: &str) -> Option<u64> {
    let src = wisp_build::read_source(&root.join(rel)).ok()?;
    Some(quick::code_hash(&src))
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

/// The dialog's sentence: where to look.
fn summary(first: Option<(String, usize)>, count: usize) -> String {
    match first {
        Some((file, line)) if count > 1 => {
            format!("{count} errors. The first is in {file} on line {line}.")
        }
        Some((file, line)) => format!("Error in {file} on line {line}."),
        None => String::new(),
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
    let mut next = match wisp_build::hot(&app.abs) {
        Ok(next) => next,
        Err(e) => return Swap::Invalid(e),
    };
    next.code = String::new();
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
    /// `root` from the top of the file system.
    abs: PathBuf,
    port: u16,
    events_port: u16,
    /// Ports the app may move on by when its own is taken; 0 once it is up.
    tries: u16,
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
            .env("WISP_PORT_TRIES", self.tries.to_string())
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
        // Startup is the app's own work (`init`, its self-tests) plus, on
        // Windows, a scan of the new executable: a busy machine can take
        // more than a few seconds. Its death ends the wait at once (the
        // pipe closes), so only a hung app waits out the whole minute.
        let mut got = listening.recv_timeout(Duration::from_secs(10));
        if matches!(got, Err(RecvTimeoutError::Timeout)) {
            term::warn("the app is not listening after 10s, still waiting");
            got = listening.recv_timeout(Duration::from_secs(50));
        }
        match got {
            Ok(addr) => {
                let at = reachable(addr.unwrap_or(SocketAddr::from(([127, 0, 0, 1], self.port))));
                if self.tries > 0 && self.port != 0 && at.port() != self.port {
                    term::warn(&format!(
                        "port {} is in use, using {}",
                        self.port,
                        at.port()
                    ));
                }
                // Later starts keep this port, waiting for the old app to let go.
                if self.port != 0 {
                    self.port = at.port();
                }
                self.tries = 0;
                self.addr = Some(at);
                Ok(())
            }
            Err(RecvTimeoutError::Timeout) => Err(
                "The app did not start listening within 60s.\nIts output is in the terminal."
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

/// Whether the app at `addr` answers an HTTP request, tried for up to
/// `wait`. Its own script (served without touching any route) is the request.
fn healthy(addr: SocketAddr, wait: Duration) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        if request(addr, "GET", "/_app/wisp-dev.js", &[]).is_some() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(Duration::from_millis(25));
    }
}

/// A minimal HTTP/1.1 request to the app; returns the status. The connect
/// timeout keeps a dead app from costing Windows' 2s refusal.
fn request(addr: SocketAddr, method: &str, path: &str, body: &[u8]) -> Option<u16> {
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(250)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let head = format!(
        "{method} {path} HTTP/1.1\r\nhost: {addr}\r\nx-wisp-dev: 1\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
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
                if !skip_dir(&e.file_name().to_string_lossy()) {
                    walk(root, &path, out);
                }
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
    // Listed, not opened one by one: a folder listing carries each entry's
    // mtime and size, where a metadata call per file opens it (on Windows).
    for dir in ["", ".wisp"] {
        let Ok(entries) = fs::read_dir(root.join(dir)) else {
            continue;
        };
        for e in entries.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            let rel = if dir.is_empty() {
                name.to_string()
            } else {
                format!("{dir}/{name}")
            };
            if !top.iter().chain(&css::POSTCSS_CONFIGS).any(|f| *f == rel) {
                continue;
            }
            // A listing describes a symlink itself: follow it, as the build does.
            let meta = match e.file_type() {
                Ok(t) if t.is_symlink() => fs::metadata(e.path()),
                _ => e.metadata(),
            };
            if let Ok(meta) = meta
                && meta.is_file()
                && let Ok(m) = meta.modified()
            {
                out.insert(rel, stamp(&e.path(), m, meta.len()));
            }
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

/// A folder never part of the app's source (dot folders, build output,
/// npm packages): skipped so a 50 ms poll does not walk thousands of files.
/// `.well-known` is served from `static/`, so it is watched.
fn skip_dir(name: &str) -> bool {
    (name.starts_with('.') && name != ".well-known") || name == "target" || name == "node_modules"
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

    fn temp(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("wisp-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src/routes")).unwrap();
        root
    }

    fn plain() -> css::Css {
        css::Css {
            tool: None,
            postcss: false,
        }
    }

    #[test]
    fn only_own_sources_skip_cargo() {
        let c = |v: &[&str]| -> Vec<(String, Change)> {
            v.iter()
                .map(|p| (p.to_string(), Change::Modified))
                .collect()
        };
        assert!(own_sources(&c(&[
            "src/routes/+page.rs",
            "src/routes/a/+page.wisp"
        ])));
        assert!(own_sources(&c(&["src/state.rs"])));
        // What cargo or the build script reads besides: it is cargo's.
        for f in [
            "Cargo.toml",
            "Cargo.lock",
            "build.rs",
            ".env",
            "package.json",
            "src/app.html",
            "src/routes/words.txt",
            "src/routes/post.md",
            "tests/a.rs",
        ] {
            assert!(!own_sources(&c(&["src/routes/+page.rs", f])), "{f}");
        }
    }

    #[test]
    fn plans_follow_the_change() {
        let c = |v: &[(&str, Change)]| -> Vec<(String, Change)> {
            v.iter().map(|(p, k)| (p.to_string(), *k)).collect()
        };
        let style = plain();
        let edit = c(&[("src/routes/+page.wisp", Change::Modified)]);
        let p = plan(&edit, &style);
        assert_eq!(
            (p.rebuild, p.templates),
            (false, vec!["src/routes/+page.wisp"])
        );
        // Rename: a remove and an add, so a new route table.
        let rename = c(&[
            ("src/routes/a/+page.wisp", Change::Removed),
            ("src/routes/b/+page.wisp", Change::Added),
        ]);
        assert!(plan(&rename, &style).rebuild);
        for rs in ["src/routes/+page.rs", "src/routes/api/+server.rs"] {
            for k in [Change::Modified, Change::Removed, Change::Added] {
                assert!(plan(&c(&[(rs, k)]), &style).rebuild, "{rs} {k:?}");
            }
        }
        let css = c(&[("src/app.css", Change::Modified)]);
        let css = plan(&css, &style);
        assert!(css.css && !css.rebuild);
        let st = c(&[("static/a.png", Change::Removed)]);
        let st = plan(&st, &style);
        assert!(st.full && !st.rebuild);
        // A compile wins over a swap in the same batch.
        let mixed = c(&[
            ("src/routes/+page.wisp", Change::Modified),
            ("src/routes/+page.rs", Change::Modified),
        ]);
        assert!(plan(&mixed, &style).rebuild);
        // The shell swaps like a template; added, it is a compile.
        let shell = c(&[("src/app.html", Change::Modified)]);
        assert_eq!(plan(&shell, &style).templates, vec!["src/app.html"]);
        assert!(plan(&c(&[("src/app.html", Change::Added)]), &style).rebuild);
        // A CSS tool's output swaps; its config is the tool's to read.
        assert!(plan(&c(&[(".wisp/app.css", Change::Modified)]), &style).css);
        let cfg = c(&[("postcss.config.js", Change::Modified)]);
        assert_eq!(plan(&cfg, &style), Plan::default());
        for f in ["Cargo.toml", ".env", "build.rs", "src/lib/x.rs"] {
            assert!(plan(&c(&[(f, Change::Modified)]), &style).rebuild, "{f}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn scan_follows_a_symlinked_top_file() {
        let root = temp("link");
        fs::write(root.join("real.toml"), "x").unwrap();
        std::os::unix::fs::symlink(root.join("real.toml"), root.join("Cargo.toml")).unwrap();
        let files = scan(&root);
        let _ = fs::remove_dir_all(&root);
        assert!(files.contains_key("Cargo.toml"));
    }

    #[test]
    fn rapid_saves_delete_and_temp_files() {
        let root = temp("watch");
        let page = root.join("src/routes/+page.wisp");
        fs::write(&page, "<p>a</p>").unwrap();
        let first = scan(&root);
        // Same size, at once: inside the clock tick, only the hash differs.
        fs::write(&page, "<p>b</p>").unwrap();
        let second = scan(&root);
        assert_eq!(
            diff(&first, &second),
            vec![("src/routes/+page.wisp".to_string(), Change::Modified)]
        );
        // Editors' scratch files are never changes.
        for f in [
            ".#+page.wisp",
            "+page.wisp~",
            ".+page.wisp.swp",
            "4913",
            "x.tmp",
        ] {
            fs::write(root.join("src/routes").join(f), "x").unwrap();
        }
        assert!(diff(&second, &scan(&root)).is_empty());
        // Save then rename: the old path goes, the new comes.
        fs::rename(&page, root.join("src/routes/+layout.wisp")).unwrap();
        let mut d = diff(&second, &scan(&root));
        d.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            d,
            vec![
                ("src/routes/+layout.wisp".to_string(), Change::Added),
                ("src/routes/+page.wisp".to_string(), Change::Removed),
            ]
        );
        let renamed = scan(&root);
        fs::remove_file(root.join("src/routes/+layout.wisp")).unwrap();
        assert_eq!(
            diff(&renamed, &scan(&root)),
            vec![("src/routes/+layout.wisp".to_string(), Change::Removed)]
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_skips_build_and_package_folders() {
        let root = std::env::temp_dir().join(format!("wisp-scan-{}", std::process::id()));
        for d in [
            "src/routes",
            "src/node_modules/x",
            "src/target",
            "static/.cache",
            "static/.well-known",
        ] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        for f in [
            "src/routes/+page.wisp",
            "src/node_modules/x/a.js",
            "src/target/b",
            "static/.cache/c",
            "static/.well-known/security.txt",
        ] {
            fs::write(root.join(f), "x").unwrap();
        }
        fs::create_dir_all(root.join(".wisp")).unwrap();
        for f in [
            "Cargo.toml",
            ".env",
            ".wisp/app.css",
            "README.md",
            ".wisp/other",
        ] {
            fs::write(root.join(f), "x").unwrap();
        }
        let files = scan(&root);
        let _ = fs::remove_dir_all(&root);
        let mut names: Vec<_> = files.keys().map(String::as_str).collect();
        names.sort();
        assert_eq!(
            names,
            [
                ".env",
                ".wisp/app.css",
                "Cargo.toml",
                "src/routes/+page.wisp",
                "static/.well-known/security.txt"
            ]
        );
    }

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
    fn health_waits_for_an_answer_not_for_a_connection() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        // Connects at once (the backlog takes it), answers 300ms later: an
        // app that is listening but not serving yet.
        let app = std::thread::spawn(move || {
            sleep(Duration::from_millis(300));
            if let Ok((mut s, _)) = listener.accept() {
                let mut head = [0u8; 512];
                let _ = s.read(&mut head);
                let _ = s.write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                );
            }
        });
        let started = Instant::now();
        assert!(healthy(addr, Duration::from_secs(10)));
        assert!(started.elapsed() >= Duration::from_millis(250));
        app.join().unwrap();
        // Nothing there: false once the time is up, not before.
        let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let gone = dead.local_addr().unwrap();
        drop(dead);
        let started = Instant::now();
        assert!(!healthy(gone, Duration::from_millis(200)));
        assert!(started.elapsed() >= Duration::from_millis(200));
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
