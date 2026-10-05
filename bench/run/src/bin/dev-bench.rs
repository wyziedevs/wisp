//! `wisp dev` speed: scaffolds the demo app with `wisp new`, then times,
//! N runs each, the median of
//!
//! - cold: the first `wisp dev` on a fresh app (dependencies too), once;
//! - start: `wisp dev` again, warm, to the first 200 for `/`;
//! - one edit of each kind, saved to the terminal's verdict and a 200:
//!   markup, a page's script, CSS, `+page.rs`, `+server.rs`;
//! - idle: the CPU `wisp dev` (the watcher) uses in 30 s of no edits.
//!
//! Wall time, and the `wisp dev` process's own CPU time (its children's,
//! cargo's and rustc's, are gone when they exit and are not counted).
//!
//! `--ab` compares the dev profile with and without the
//! `[profile.dev.package."*"]` block, alternating A and B in each run so
//! that noise from other work hits both alike.
//!
//! ```text
//! cargo build -p wisp-web
//! cargo run -q -p bench-run --release --bin dev-bench -- [--runs 5] [--ab] [--wisp path/to/wisp]
//! ```

#[allow(dead_code)]
#[path = "../sys.rs"]
mod sys;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

/// An edit: the file, and the text it toggles between; the terminal's
/// word that says it is applied.
struct Edit {
    file: &'static str,
    a: &'static str,
    b: &'static str,
    done: &'static str,
}

const EDITS: [Edit; 5] = [
    Edit {
        file: "src/routes/about/+page.wisp",
        a: "<h1>About Wisp</h1>",
        b: "<h1>About Wisp!</h1>",
        done: "swapped in",
    },
    Edit {
        file: "src/routes/+page.wisp",
        a: "let x = 0, y = 0",
        b: "let x = 1, y = 0",
        done: "swapped in",
    },
    Edit {
        file: "src/app.css",
        a: "--paper: #141414;",
        b: "--paper: #151515;",
        done: "styles swapped",
    },
    Edit {
        file: "src/routes/wisple/+page.rs",
        a: "const TRIES: usize = 6;",
        b: "const TRIES: usize = 7;",
        done: "Rebuilt",
    },
    Edit {
        file: "src/routes/ping/+server.rs",
        a: "\"a\"",
        b: "\"b\"",
        done: "Rebuilt",
    },
];

const PROFILE_DEPS: &str = "[profile.dev.package.\"*\"]\ndebug = false\n";

struct Dev {
    child: Child,
    lines: Receiver<String>,
    port: u16,
}

impl Dev {
    fn start(wisp: &Path, app: &Path, port: u16) -> Dev {
        let mut child = Command::new(wisp)
            .args(["dev", "--port", &port.to_string()])
            .current_dir(app)
            .env("CARGO_TARGET_DIR", app.join("target"))
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start wisp dev");
        let (tx, lines) = mpsc::channel();
        for r in [
            Box::new(child.stdout.take().unwrap()) as Box<dyn Read + Send>,
            Box::new(child.stderr.take().unwrap()),
        ] {
            let tx = tx.clone();
            std::thread::spawn(move || {
                for l in BufReader::new(r).lines().map_while(Result::ok) {
                    let _ = tx.send(l);
                }
            });
        }
        Dev { child, lines, port }
    }

    /// Waits for a line containing `word`; panics on a failed build.
    fn wait(&self, word: &str, limit: Duration) {
        let deadline = Instant::now() + limit;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = self
                .lines
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("no `{word}` from wisp dev"));
            if line.contains(word) {
                return;
            }
            if line.contains("failed") || line.contains("Failed") {
                panic!("wisp dev: {line}");
            }
        }
    }

    fn drain(&self) {
        while self.lines.try_recv().is_ok() {}
    }

    /// Until `/` answers 200 (the app may move to the next port).
    fn ok(&self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            for p in self.port..self.port + 20 {
                if status(p) == Some(200) {
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("no 200 for /");
    }

    fn cpu(&self) -> f64 {
        let id = self.child.id();
        sys::cpu_times(&[id]).get(&id).map_or(0.0, |t| t.0)
    }

    fn stop(mut self) {
        sys::kill(&mut self.child);
    }
}

fn status(port: u16) -> Option<u16> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(100)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    s.write_all(b"GET / HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n")
        .ok()?;
    let mut head = [0u8; 12];
    s.read_exact(&mut head).ok()?;
    std::str::from_utf8(&head[9..12]).ok()?.parse().ok()
}

/// Wall and CPU seconds of each sample of one scenario.
#[derive(Default)]
struct Samples {
    wall: Vec<f64>,
    cpu: Vec<f64>,
}

fn median(v: &[f64]) -> f64 {
    let mut v = v.to_vec();
    v.sort_by(f64::total_cmp);
    if v.is_empty() { 0.0 } else { v[v.len() / 2] }
}

/// Flips the file to the other text; `reset` only puts back `a`.
fn toggle(app: &Path, e: &Edit, reset: bool) {
    let path = app.join(e.file);
    let text = std::fs::read_to_string(&path).expect(e.file);
    let (from, to) = if text.contains(e.a) {
        (e.a, e.b)
    } else {
        (e.b, e.a)
    };
    assert!(text.contains(from), "{}: no {from}", e.file);
    if reset && from == e.a {
        return;
    }
    std::fs::write(&path, text.replacen(from, to, 1)).expect(e.file);
}

fn scaffold(wisp: &Path, dir: &Path, name: &str, deps_profile: bool) -> PathBuf {
    let app = dir.join(name);
    if app.join("target").is_dir() {
        return app;
    }
    let _ = std::fs::remove_dir_all(&app);
    let ok = Command::new(wisp)
        .args([
            "new",
            name,
            "-y",
            "--template",
            "demo",
            "--no-tailwind",
            "--no-git",
            "--no-install",
        ])
        .current_dir(dir)
        .status()
        .expect("wisp new");
    assert!(ok.success(), "wisp new failed");
    let ping = app.join("src/routes/ping");
    std::fs::create_dir_all(&ping).unwrap();
    std::fs::write(
        ping.join("+server.rs"),
        "fn get() -> &'static str {\n    \"a\"\n}\n",
    )
    .unwrap();
    let toml = app.join("Cargo.toml");
    let text = std::fs::read_to_string(&toml).unwrap();
    let has = text.contains(PROFILE_DEPS);
    let text = match (deps_profile, has) {
        (true, false) => text.replacen(
            "[profile.release]",
            &format!("{PROFILE_DEPS}\n[profile.release]"),
            1,
        ),
        (false, true) => text.replacen(PROFILE_DEPS, "", 1),
        _ => text,
    };
    std::fs::write(&toml, text).unwrap();
    app
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |k: &str| {
        args.iter()
            .position(|a| a == k)
            .and_then(|i| args.get(i + 1))
    };
    let runs: usize = arg("--runs").and_then(|r| r.parse().ok()).unwrap_or(5);
    let ab = args.iter().any(|a| a == "--ab");
    let idle_secs: u64 = arg("--idle").and_then(|r| r.parse().ok()).unwrap_or(30);
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let wisp = arg("--wisp").map(PathBuf::from).unwrap_or_else(|| {
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| repo.join("target"));
        target
            .join("debug")
            .join(format!("wisp{}", std::env::consts::EXE_SUFFIX))
    });
    let wisp = wisp
        .canonicalize()
        .expect("the wisp binary: cargo build -p wisp-web");
    let dir = std::env::temp_dir().join("wisp-dev-bench");
    std::fs::create_dir_all(&dir).unwrap();

    // Variant B (the shipped profile) always; A (no deps block) with --ab.
    // `--base old/wisp`: the same app under an older `wisp` (A) and this one.
    let base = arg("--base").map(|b| PathBuf::from(b).canonicalize().expect("--base"));
    let variants: Vec<(&str, bool, &Path)> = match (&base, ab) {
        (Some(b), _) => vec![("A base wisp", true, b), ("B this wisp", true, &wisp)],
        (None, true) => vec![
            ("A no-deps-block", false, &wisp),
            ("B deps debug=false", true, &wisp),
        ],
        (None, false) => vec![("current", true, &wisp)],
    };
    // App 0 lacks the deps block, app 1 has it.
    let apps: Vec<PathBuf> = variants
        .iter()
        .map(|v| scaffold(&wisp, &dir, &format!("app{}", v.1 as u8), v.1))
        .collect();

    let mut port = 4300;
    let mut cold = vec![None; apps.len()];
    for (i, app) in apps.iter().enumerate() {
        if app.join("target/debug").is_dir() {
            continue;
        }
        let wisp = variants[i].2;
        let t = Instant::now();
        let dev = Dev::start(wisp, app, port);
        dev.wait("Ready", Duration::from_secs(1800));
        dev.ok();
        cold[i] = Some(t.elapsed().as_secs_f64());
        dev.stop();
        port += 25;
    }

    let names = [
        "start",
        "markup",
        "script",
        "css",
        "page.rs",
        "server.rs",
        "idle",
    ];
    let mut res: Vec<Vec<Samples>> = apps
        .iter()
        .map(|_| names.iter().map(|_| Samples::default()).collect())
        .collect();
    for run in 0..runs {
        for (i, app) in apps.iter().enumerate() {
            let s = &mut res[i];
            let t = Instant::now();
            let dev = Dev::start(variants[i].2, app, port);
            dev.wait("Ready", Duration::from_secs(600));
            dev.ok();
            s[0].wall.push(t.elapsed().as_secs_f64());
            s[0].cpu.push(dev.cpu());
            for (k, e) in EDITS.iter().enumerate() {
                std::thread::sleep(Duration::from_millis(300));
                dev.drain();
                let c = dev.cpu();
                let t = Instant::now();
                toggle(app, e, false);
                dev.wait(e.done, Duration::from_secs(600));
                dev.ok();
                s[k + 1].wall.push(t.elapsed().as_secs_f64());
                s[k + 1].cpu.push(dev.cpu() - c);
            }
            if idle_secs > 0 {
                let c = dev.cpu();
                std::thread::sleep(Duration::from_secs(idle_secs));
                s[6].wall.push(idle_secs as f64);
                s[6].cpu.push(dev.cpu() - c);
            }
            dev.stop();
            port += 25;
            eprintln!("run {} {} done", run + 1, variants[i].0);
        }
    }
    // Leave the files as scaffolded for the next invocation.
    for app in &apps {
        for e in &EDITS {
            toggle(app, e, true);
        }
    }

    println!("wisp dev, median of {runs} runs (wall s / wisp dev CPU s)");
    for (i, v) in variants.iter().enumerate() {
        println!("\n{}", v.0);
        if let Some(c) = cold[i] {
            println!("  {:<10} {c:>8.2}  (once, fresh app)", "cold");
        }
        for (k, n) in names.iter().enumerate() {
            let s = &res[i][k];
            println!(
                "  {n:<10} {:>8.3}  {:>8.3}",
                median(&s.wall),
                median(&s.cpu)
            );
        }
    }
}
