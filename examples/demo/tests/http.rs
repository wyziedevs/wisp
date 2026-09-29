//! The demo app's binary, run as a server and sent what browsers, proxies
//! and attackers send: ordinary requests, malformed ones and hostile ones.
//! Every answer is checked, and after each the server must still answer.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// The server, stopped when dropped.
struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start() -> Server {
    let mut child = Command::new(env!("CARGO_BIN_EXE_demo"))
        .env("PORT", "0")
        .env("HOST", "127.0.0.1")
        .env("WISP_THREADS", "2")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start the demo");
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let port = line
        .trim()
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(|| panic!("no port in {line:?}"));
    Server { child, port }
}

impl Server {
    /// Sends `raw` as is and returns everything that comes back until the
    /// server closes the connection.
    fn send(&self, raw: &[u8]) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(raw).unwrap();
        let mut got = Vec::new();
        let _ = s.read_to_end(&mut got);
        String::from_utf8_lossy(&got).into_owned()
    }

    /// One request that closes the connection when answered.
    fn request(&self, method: &str, target: &str, headers: &str, body: &str) -> String {
        let raw = format!(
            "{method} {target} HTTP/1.1\r\nhost: 127.0.0.1:{}\r\nconnection: close\r\ncontent-length: {}\r\n{headers}\r\n{body}",
            self.port,
            body.len()
        );
        self.send(raw.as_bytes())
    }

    fn alive(&self) {
        assert!(
            status(&self.request("GET", "/", "", "")) == 200,
            "the server stopped answering"
        );
    }
}

fn status(response: &str) -> u16 {
    response
        .get(9..12)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

fn header<'a>(response: &'a str, name: &str) -> Option<&'a str> {
    let head = response.split("\r\n\r\n").next()?;
    head.lines().find_map(|l| {
        l.split_once(": ")
            .filter(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    })
}

#[test]
fn pages_files_and_redirects() {
    let s = start();
    let home = s.request("GET", "/", "", "");
    assert_eq!(status(&home), 200);
    assert!(home.contains("<script defer src=\"/_app/wisp.js"));
    assert!(
        home.contains("<script type=\"module\" src=\"/_app/live.js"),
        "the page's client script is started by the runtime"
    );
    assert!(
        !home.contains("mascot.getBoundingClientRect"),
        "the client script is a module of its own, not in the page"
    );

    let head = s.request("HEAD", "/", "", "");
    assert_eq!(status(&head), 200);
    assert!(head.ends_with("\r\n\r\n"), "HEAD has no body");

    assert_eq!(status(&s.request("GET", "/nope", "", "")), 404);
    let slash = s.request("GET", "/about/?x=1", "", "");
    assert_eq!(
        (status(&slash), header(&slash, "location")),
        (308, Some("/about?x=1"))
    );
    assert_eq!(status(&s.request("GET", "/favicon.svg", "", "")), 200);
    assert_eq!(status(&s.request("GET", "/_app/wisp.js", "", "")), 200);
    assert_eq!(status(&s.request("GET", &"/a".repeat(40), "", "")), 404);
}

#[test]
fn static_files_stay_in_static() {
    let s = start();
    for path in [
        "/../Cargo.toml",
        "/%2e%2e/Cargo.toml",
        "/..%2fCargo.toml",
        "/%2e%2e%5cCargo.toml",
        "/static/../Cargo.toml",
        "//etc/passwd",
    ] {
        let r = s.request("GET", path, "", "");
        assert!(
            !r.contains("[package]"),
            "{path} served a file outside static/"
        );
        assert_ne!(status(&r), 200, "{path}");
    }
    s.alive();
}

#[test]
fn malformed_requests_are_refused() {
    let s = start();
    // Each is answered with the status, and the connection is closed.
    let cases: [(&[u8], u16); 8] = [
        (b"GARBAGE\r\n\r\n", 400),
        (b"GET / HTTP/1.1\r\nhost x\r\n\r\n", 400),
        (
            b"GET / HTTP/1.1\r\ncontent-length: 1\r\ncontent-length: 2\r\n\r\nab",
            400,
        ),
        (b"GET / HTTP/1.1\r\ncontent-length: -1\r\n\r\n", 400),
        (b"GET / HTTP/1.1\r\ncontent-length: 1 1\r\n\r\n", 400),
        (
            b"POST / HTTP/1.1\r\ntransfer-encoding: chunked\r\ncontent-length: 3\r\n\r\n0\r\n\r\n",
            400,
        ),
        (
            b"POST / HTTP/1.1\r\ntransfer-encoding: gzip\r\n\r\n0\r\n\r\n",
            501,
        ),
        (b"POST / HTTP/1.1\r\ncontent-length: 99999999\r\n\r\n", 413),
    ];
    for (raw, want) in cases {
        let r = s.send(raw);
        assert_eq!(
            status(&r),
            want,
            "{:?} got {r:?}",
            String::from_utf8_lossy(raw)
        );
        assert_eq!(header(&r, "connection"), Some("close"));
    }
    for target in ["*", "http://example.com/"] {
        assert_eq!(status(&s.request("GET", target, "", "")), 400, "{target}");
    }
    assert_eq!(
        status(&s.send(b"GET /\xff HTTP/1.1\r\nconnection: close\r\n\r\n")),
        400
    );
    assert_eq!(status(&s.request("GET", "/\u{ff}", "", "")), 404);

    let big = format!("GET / HTTP/1.1\r\nx-big: {}\r\n\r\n", "a".repeat(20_000));
    assert_eq!(status(&s.send(big.as_bytes())), 431);
    let many: String = (0..100).map(|i| format!("x-{i}: 1\r\n")).collect();
    assert_eq!(
        status(&s.send(format!("GET / HTTP/1.1\r\n{many}\r\n").as_bytes())),
        431
    );
    assert_eq!(status(&s.request("BREW", "/", "", "")), 405);
    s.alive();
}

#[test]
fn keep_alive_pipelining_and_continue() {
    let s = start();
    let two = s.send(b"GET /about HTTP/1.1\r\nhost: x\r\n\r\nGET /nope HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n");
    assert_eq!(two.matches("HTTP/1.1 200 OK").count(), 1, "{two}");
    assert_eq!(two.matches("HTTP/1.1 404").count(), 1, "{two}");

    let mut c = TcpStream::connect(("127.0.0.1", s.port)).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    c.write_all(b"POST /?/increment HTTP/1.1\r\nhost: x\r\nexpect: 100-continue\r\ncontent-length: 1\r\nconnection: close\r\n\r\n").unwrap();
    let mut first = [0; 25];
    c.read_exact(&mut first).unwrap();
    assert_eq!(&first, b"HTTP/1.1 100 Continue\r\n\r\n");
    c.write_all(b"x").unwrap();
    let mut rest = String::new();
    let _ = c.read_to_string(&mut rest);
    assert_eq!(status(&rest), 200);
}

#[test]
fn actions() {
    let s = start();
    let form = "content-type: application/x-www-form-urlencoded\r\n";
    let r = s.request("POST", "/?/increment", form, "");
    assert_eq!(status(&r), 200);
    assert!(
        header(&r, "set-cookie").is_some_and(|c| c.starts_with("count=1;")),
        "{r}"
    );

    let foreign = format!("{form}origin: https://evil.example\r\n");
    assert_eq!(
        status(&s.request("POST", "/?/increment", &foreign, "")),
        403
    );
    assert_eq!(status(&s.request("POST", "/?/nope", form, "")), 404);
    assert_eq!(status(&s.request("DELETE", "/", "", "")), 405);

    // A guess that is not five letters, even when its bytes number five.
    assert_eq!(
        status(&s.request("POST", "/wisple?/enter", form, "guess=%C3%A9%C3%A9a")),
        400
    );
    assert_eq!(status(&s.request("POST", "/wisple?/enter", form, "")), 400);
    s.alive();
}

#[test]
fn cookies_are_visitor_input() {
    let s = start();
    for cookie in [
        "count=junk",
        "count=99999999999999999999",
        "count=",
        "wisple=|||||",
        "wisple=%ZZ|x",
        "wisple=9999|aaaaa|b",
        "=;;=;",
    ] {
        let r = s.request("GET", "/", &format!("cookie: {cookie}\r\n"), "");
        assert_eq!(status(&r), 200, "cookie {cookie}");
        let r = s.request("GET", "/wisple", &format!("cookie: {cookie}\r\n"), "");
        assert_eq!(status(&r), 200, "cookie {cookie}");
    }
    s.alive();
}

/// SIGTERM stops taking connections, answers the request under way, and
/// exits cleanly.
#[cfg(unix)]
#[test]
fn stops_gracefully() {
    let mut s = start();
    let mut c = TcpStream::connect(("127.0.0.1", s.port)).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    c.write_all(b"POST /?/increment HTTP/1.1\r\nhost: x\r\ncontent-length: 4\r\n\r\nab")
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    Command::new("kill")
        .args(["-TERM", &s.child.id().to_string()])
        .status()
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        TcpStream::connect(("127.0.0.1", s.port)).is_err(),
        "still accepting"
    );
    c.write_all(b"cd").unwrap();
    let mut r = String::new();
    let _ = c.read_to_string(&mut r);
    assert_eq!(status(&r), 200);
    assert_eq!(header(&r, "connection"), Some("close"));
    assert!(s.child.wait().unwrap().success());
}
