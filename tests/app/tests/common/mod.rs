//! What the tests that run the test app's binary share: starting it, its
//! port, raw requests to it, and a temp path that cleans up after itself
//! (`Temp`).
// Each test file uses part of this.
#![allow(dead_code, unused_imports)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

#[path = "../../../../tests/shared/temp.rs"]
mod temp;

pub use temp::Temp;

pub const SECRET: &str = "0123456789abcdef0123456789abcdef";

/// The server, killed when dropped.
pub struct Server {
    pub child: Child,
    pub port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Gives `cmd` the usual settings (a free port, no saved data), then `env` on top.
pub fn configure<'a>(cmd: &'a mut Command, env: &[(&str, &str)]) -> &'a mut Command {
    cmd.env("PORT", "0")
        .env("HOST", "127.0.0.1")
        .env("WISP_THREADS", "2")
        .env("WISP_SECRET", SECRET)
        .env("WISP_DATA", "off")
        .envs(env.iter().copied())
}

/// The app's binary with `env`.
pub fn command(env: &[(&str, &str)]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wisp-test-app"));
    configure(&mut cmd, env);
    cmd
}

/// Runs `cmd` (the app, or a shell that runs it) and waits for its port.
pub fn spawn(cmd: Command) -> Server {
    try_spawn(cmd).expect("the test app printed no port")
}

/// Like `spawn`, but `None` when the app exits without printing its port (it could not bind).
pub fn try_spawn(mut cmd: Command) -> Option<Server> {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("start the test app");
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let mut server = Server { child, port: 0 };
    server.port = line.trim().rsplit(':').next()?.parse().ok()?;
    Some(server)
}

/// The app, started with `env`.
pub fn start(env: &[(&str, &str)]) -> Server {
    spawn(command(env))
}

impl Server {
    /// Sends `raw` as is and returns everything that comes back until the
    /// server closes the connection.
    pub fn send(&self, raw: &[u8]) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(raw).unwrap();
        let mut got = Vec::new();
        let _ = s.read_to_end(&mut got);
        String::from_utf8_lossy(&got).into_owned()
    }

    /// One request that closes the connection when answered.
    pub fn request(&self, method: &str, target: &str, headers: &str, body: &[u8]) -> String {
        self.request_declaring(method, target, headers, body.len(), body)
    }

    /// A request that says its body is `declared` long, and sends `body`, all in one write.
    /// A body too large for the route is only announced: a client that is told no does not
    /// send it, and a server that closes with unread bytes resets the connection.
    pub fn request_declaring(
        &self,
        method: &str,
        target: &str,
        headers: &str,
        declared: usize,
        body: &[u8],
    ) -> String {
        let mut raw = format!(
            "{method} {target} HTTP/1.1\r\nhost: 127.0.0.1:{}\r\nconnection: close\r\ncontent-length: {declared}\r\n{headers}\r\n",
            self.port
        )
        .into_bytes();
        raw.extend_from_slice(body);
        self.send(&raw)
    }
}

/// A connection to `port`, which gives up waiting for an answer after 5 s.
pub fn connect(port: u16) -> TcpStream {
    let c = TcpStream::connect(("127.0.0.1", port)).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    c
}

pub fn status(response: &str) -> u16 {
    response
        .get(9..12)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

pub fn header<'a>(response: &'a str, name: &str) -> Option<&'a str> {
    let head = response.split("\r\n\r\n").next()?;
    head.lines().skip(1).find_map(|l| {
        l.split_once(": ")
            .filter(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    })
}

pub fn body(response: &str) -> &str {
    response.split_once("\r\n\r\n").map_or("", |(_, b)| b)
}

/// The content type of a `multipart` body.
pub const MULTIPART: &str = "multipart/form-data; boundary=XX";

/// A form as a browser posts it with a file input: `(name, filename, bytes)`
/// parts, a file (said to be a PNG) where there is a filename.
pub fn multipart(parts: &[(&str, Option<&str>, &[u8])]) -> Vec<u8> {
    let mut b = Vec::new();
    for (name, file, bytes) in parts {
        let head = match file {
            Some(f) => format!(
                "--XX\r\ncontent-disposition: form-data; name=\"{name}\"; filename=\"{f}\"\r\ncontent-type: image/png\r\n\r\n"
            ),
            None => format!("--XX\r\ncontent-disposition: form-data; name=\"{name}\"\r\n\r\n"),
        };
        b.extend_from_slice(head.as_bytes());
        b.extend_from_slice(bytes);
        b.extend_from_slice(b"\r\n");
    }
    b.extend_from_slice(b"--XX--\r\n");
    b
}

/// The status line and headers of the next answer, without the blank line.
pub fn read_head(c: &mut BufReader<TcpStream>) -> String {
    let mut head = String::new();
    loop {
        let mut line = String::new();
        c.read_line(&mut line).unwrap();
        if line == "\r\n" {
            return head;
        }
        assert!(
            !line.is_empty(),
            "the connection closed inside a head: {head:?}"
        );
        head.push_str(&line);
    }
}

/// One answer: the head, and the body if it says how long it is.
pub fn read_answer(c: &mut BufReader<TcpStream>) -> (String, String) {
    let head = read_head(c);
    let len = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length: ")?
                .parse()
                .ok()
        })
        .unwrap_or(0);
    let mut body = vec![0; len];
    c.read_exact(&mut body).unwrap();
    (head, String::from_utf8_lossy(&body).into_owned())
}
