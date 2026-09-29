//! `wisp dev`: build, serve, and follow the files.
//!
//! The server only stops when its terminal goes away, which is what the test
//! ends with: nothing reads its output, so the next line it prints fails and
//! it exits, and its app with it.

use crate::{Dir, command, fake_tailwind, has, pinned_app, write};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

/// A running `wisp dev`, and every line it and the app print.
struct Dev {
    child: Child,
    lines: Receiver<String>,
    log: String,
    /// Tells the reader of its stdout to close it at the next line.
    hang_up: Arc<AtomicBool>,
}

fn pump(from: impl Read + Send + 'static, to: Sender<String>, hang_up: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        for line in BufReader::new(from).lines().map_while(Result::ok) {
            if hang_up.load(Ordering::Relaxed) || to.send(line).is_err() {
                return;
            }
        }
    });
}

impl Dev {
    fn start(app: &Path, args: &[&str], env: &[(&str, &Path)]) -> Dev {
        let mut child = command(app)
            .arg("dev")
            .args(args)
            .envs(env.iter().copied())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let (to, lines) = channel();
        let hang_up = Arc::new(AtomicBool::new(false));
        pump(child.stdout.take().unwrap(), to.clone(), hang_up.clone());
        // Only stdout is closed on hanging up; it is where changes are told.
        pump(child.stderr.take().unwrap(), to, Arc::default());
        Dev {
            child,
            lines,
            log: String::new(),
            hang_up,
        }
    }

    /// The next line with `needle` in it, skipping the ones before. A first
    /// build compiles everything, so it may take a while; a hang would not.
    fn wait_for(&mut self, needle: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(240);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) if line.contains(needle) => return line,
                Ok(line) => {
                    self.log.push_str(&line);
                    self.log.push('\n');
                }
                Err(_) => panic!("no {needle:?} in the output:\n{}", self.log),
            }
        }
    }

    /// Closes the terminal: touches files until the next line it prints
    /// finds nobody reading, and it exits. Returns once it has.
    fn hang_up(&mut self, app: &Path) {
        self.hang_up.store(true, Ordering::Relaxed);
        for round in 0..40 {
            write(app, "static/hang-up.txt", &round.to_string());
            for _ in 0..30 {
                if self.child.try_wait().unwrap().is_some() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        panic!("wisp dev is still running with its terminal gone");
    }
}

impl Drop for Dev {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A GET over a plain socket: the status and the whole response text.
fn get(addr: &str, path: &str) -> (u16, String) {
    let mut s = TcpStream::connect(addr).unwrap_or_else(|e| panic!("connect {addr}: {e}"));
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nhost: {addr}\r\nconnection: close\r\n\r\n"
    )
    .unwrap();
    let mut text = String::new();
    s.read_to_string(&mut text).unwrap();
    (text[9..12].parse().unwrap(), text)
}

/// The reload stream a browser holds open.
struct Events {
    stream: TcpStream,
    seen: String,
}

impl Events {
    fn open(port: &str) -> Events {
        let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(60)))
            .unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nhost: x\r\n\r\n")
            .unwrap();
        let mut events = Events {
            stream,
            seen: String::new(),
        };
        events.next("retry: 500\n\n");
        events
    }

    /// Reads until `needle` has come, and drops everything up to it.
    fn next(&mut self, needle: &str) {
        loop {
            if let Some(at) = self.seen.find(needle) {
                self.seen.drain(..at + needle.len());
                return;
            }
            let mut buf = [0u8; 4096];
            let n = self.stream.read(&mut buf).unwrap_or(0);
            assert!(n > 0, "no {needle:?} in the stream: {:?}", self.seen);
            self.seen.push_str(&String::from_utf8_lossy(&buf[..n]));
        }
    }
}

/// The address a "Ready at http://127.0.0.1:PORT in 1.2s" line names. Port 0
/// is any port, so each restart of the app is at another.
fn address(line: &str) -> String {
    let rest = line.split("http://").nth(1).unwrap();
    rest.split_whitespace().next().unwrap().to_string()
}

/// Waits for nothing to be listening at `addr` any more.
fn assert_stopped(addr: &str) {
    let sock: SocketAddr = addr.parse().unwrap();
    let gone = (0..50).any(|_| {
        let closed = TcpStream::connect_timeout(&sock, Duration::from_millis(100)).is_err();
        if !closed {
            std::thread::sleep(Duration::from_millis(100));
        }
        closed
    });
    assert!(gone, "the app is still listening at {addr}");
}

#[test]
fn builds_serves_and_follows_the_files() {
    let cwd = Dir::new("dev");
    let app = pinned_app(&cwd, "dev-site", &["-t", "minimal"]);
    let page = |text: &str| {
        let head = "<title>Home</title>
";
        write(
            &app,
            "src/routes/+page.wisp",
            &format!(
                "{head}{text}
"
            ),
        );
    };
    page("<p>{oops</p>");
    let mut dev = Dev::start(&app, &["--port", "0"], &[]);

    // A broken template is reported without compiling, and fixing it starts
    // the app.
    let e = dev.wait_for("✗");
    has(&e, &["src/routes/+page.wisp:2:4: unclosed {"]);
    page("<h1>first</h1>");
    let mut addr = address(&dev.wait_for(" at http://"));

    let (status, body) = get(&addr, "/");
    assert_eq!(status, 200);
    has(
        &body,
        &["<h1>first</h1>", "<title>Home</title>", "/_app/wisp-dev.js"],
    );
    let port = body
        .split("data-port=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let mut events = Events::open(port);
    let (status, body) = get(&addr, "/nope");
    assert!(status == 404 && body.contains("<h1>404</h1>"), "{body}");

    // Text in a template is swapped in without a compile.
    page("<h1>second</h1>");
    dev.wait_for("swapped in");
    assert!(get(&addr, "/").1.contains("<h1>second</h1>"));

    // One that no longer parses is shown in the browser, the app keeps the
    // last good page, and the next save that parses is swapped in.
    page("<p>{oops</p>");
    let e = dev.wait_for("✗");
    has(&e, &["src/routes/+page.wisp:2:4: unclosed {"]);
    events.next(
        "Build Failed
data: src/routes/+page.wisp, line 2. Save a fix and the page updates.",
    );
    events.next("Template Check");
    assert!(get(&addr, "/").1.contains("<h1>second</h1>"));
    page("<h1>third</h1>");
    dev.wait_for("swapped in");
    assert!(get(&addr, "/").1.contains("<h1>third</h1>"));

    // A static file reloads the page, a style swaps in place.
    write(&app, "static/hello.txt", "hello");
    dev.wait_for("reloaded");
    events.next(
        "data: full
",
    );
    let (status, body) = get(&addr, "/hello.txt");
    assert!(status == 200 && body.ends_with("hello"), "{body}");
    write(
        &app,
        "src/app.css",
        "body { color: red; }
",
    );
    dev.wait_for("styles swapped");
    events.next(
        "data: css
",
    );

    // A new route is a rebuild, and the new page is there after it.
    write(
        &app,
        "src/routes/about/+page.wisp",
        "<h1>About</h1>
",
    );
    addr = address(&dev.wait_for("Rebuilt at http://"));
    events.next(
        "data: reload
",
    );
    let (status, body) = get(&addr, "/about");
    assert!(status == 200 && body.contains("<h1>About</h1>"), "{body}");

    // A compile error is shown in the browser, and the old app keeps serving.
    write(
        &app,
        "src/hooks.rs",
        "fn init() {
    let x: u32 = \"a\";
}
",
    );
    dev.wait_for("Build failed with 1 error.");
    events.next(
        "Build Failed
data: src/hooks.rs, line 2.",
    );
    events.next("mismatched types");
    assert_eq!(get(&addr, "/about").0, 200);
    // Several are counted.
    write(
        &app,
        "src/hooks.rs",
        "fn init() {
    let x: u32 = \"a\";
    let y: u32 = \"b\";
}
",
    );
    dev.wait_for("Build failed with 2 errors.");
    events.next(
        "Build Failed
data: 2 errors. The first is in src/hooks.rs, line 2.",
    );
    write(
        &app,
        "src/hooks.rs",
        "fn init() {}
",
    );
    addr = address(&dev.wait_for("Rebuilt at http://"));
    events.next(
        "data: reload
",
    );
    assert_eq!(get(&addr, "/about").0, 200);

    // With its terminal gone it exits, and its app is not left on the port.
    dev.hang_up(&app);
    assert_stopped(&addr);
}

#[test]
fn tailwind_runs_beside_it() {
    let cwd = Dir::new("dev-tailwind");
    let app = pinned_app(&cwd, "dev-tw", &["-t", "minimal", "--tailwind"]);
    let tailwind = fake_tailwind(&cwd, 0);
    let mut dev = Dev::start(&app, &["--port", "0"], &[("WISP_TAILWIND", &tailwind)]);

    // Its errors are told in Wisp's words, its other lines pass through, and
    // its progress lines are dropped.
    let e = dev.wait_for("✗");
    assert!(e.contains("Tailwind could not build the CSS."), "{e}");
    dev.wait_for("boom");
    assert!(!dev.log.contains("Done in"), "{}", dev.log);
    assert!(dev.log.contains("warning: other"), "{}", dev.log);
    let addr = address(&dev.wait_for(" at http://"));
    assert!(get(&addr, "/").1.contains("/_app/app.css"));

    // What Tailwind writes swaps in; the source it is watching is left to it.
    write(&app, ".wisp/app.css", "b{color:red}");
    dev.wait_for("styles swapped");
    write(
        &app,
        "src/app.css",
        "@import \"tailwindcss\";
p { margin: 0 }
",
    );
    write(&app, ".wisp/app.css", "b{color:blue}");
    dev.wait_for("styles swapped");
    let (status, css) = get(&addr, "/_app/app.css");
    assert!(status == 200 && css.contains("b{color:blue}"), "{css}");
    dev.hang_up(&app);
    assert_stopped(&addr);
}

#[test]
fn an_app_that_cannot_listen_is_reported_and_dev_keeps_watching() {
    let cwd = Dir::new("dev-busy");
    let app = pinned_app(&cwd, "dev-busy", &["-t", "minimal"]);
    // A port somebody else holds.
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port().to_string();
    let host = Path::new("127.0.0.1");
    let mut dev = Dev::start(&app, &["-p", &port], &[("HOST", host)]);

    // The address is known and shown before the app is built.
    let banner = dev.wait_for("Ctrl+C to stop");
    assert!(
        banner.contains(&format!("http://127.0.0.1:{port}")),
        "{banner}"
    );
    let e = dev.wait_for("✗");
    has(&e, &["The app stopped while starting"]);
    dev.wait_for("a port already in use is the usual cause.");

    // Freeing the port and saving starts it.
    drop(taken);
    write(
        &app,
        "src/routes/+page.wisp",
        "<h1>Now</h1>
",
    );
    dev.wait_for("Rebuilt");
    let (status, body) = get(&format!("127.0.0.1:{port}"), "/");
    assert!(status == 200 && body.contains("<h1>Now</h1>"), "{body}");
    dev.hang_up(&app);
    assert_stopped(&format!("127.0.0.1:{port}"));
}
