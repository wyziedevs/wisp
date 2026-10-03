//! The app built for the edge (`--target node`: `app.wasm` and the shims
//! `wisp build` ships) answers a fixed set of requests as the native server
//! does: the same status, content type, redirect and body. Skipped, with a
//! note, when Node or Rust's wasm32 target is not there.

mod common;

use common::{SECRET, Server, Temp, header, status};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const FORM: &str = "content-type: application/x-www-form-urlencoded\r\n";

/// Method, target, headers, body.
const REQUESTS: [(&str, &str, &str, &str); 9] = [
    ("GET", "/", "", ""),
    ("GET", "/styled", "", ""),
    ("GET", "/nope", "", ""),
    ("GET", "/login/?a=1", "", ""),
    ("GET", "/admin", "", ""),
    ("GET", "/notes", "", ""),
    ("GET", "/_app/wisp.js", "", ""),
    ("POST", "/echo", "", "hello"),
    ("POST", "/echo", FORM, "a=1&b=two"),
];

/// A response as the wire gave it, reduced to what both servers must agree on.
fn essence(raw: &str) -> String {
    let body = common::body(raw);
    let body = if header(raw, "transfer-encoding").is_some_and(|t| t.contains("chunked")) {
        dechunk(body)
    } else {
        body.to_string()
    };
    let h = |n| header(raw, n).unwrap_or("");
    format!(
        "{} {:?} {:?}\n{body}",
        status(raw),
        h("content-type"),
        h("location")
    )
}

fn dechunk(mut s: &str) -> String {
    let mut out = String::new();
    while let Some((len, rest)) = s.split_once("\r\n") {
        let n = usize::from_str_radix(len.trim(), 16).unwrap_or(0);
        if n == 0 || rest.len() < n {
            break;
        }
        out.push_str(&rest[..n]);
        s = rest.get(n + 2..).unwrap_or("");
    }
    out
}

fn have(cmd: &mut Command) -> bool {
    cmd.stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// `cargo build` of the app for wasm32, and the folder `wisp build --target
/// node` would write, without the CLI: `None` where it cannot run.
fn node_app() -> Option<(Temp, PathBuf)> {
    if !have(Command::new("node").arg("--version")) {
        eprintln!("parity: no node, skipped");
        return None;
    }
    let target = "wasm32-unknown-unknown";
    if !have(Command::new("rustc").args(["--print", "target-libdir", "--target", target])) {
        eprintln!("parity: no {target}, skipped");
        return None;
    }
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    // This test's own `target/debug/deps/parity-*`: three up is the target folder.
    let exe = std::env::current_exe().ok()?;
    let dir = exe.ancestors().nth(3)?.to_path_buf();
    let built = Command::new(cargo)
        .args(["build", "-p", "wisp-test-app", "--target", target])
        .env("CARGO_TARGET_DIR", &dir)
        .status()
        .ok()?;
    if !built.success() {
        eprintln!("parity: no wasm build, skipped");
        return None;
    }
    let wasm = dir.join(target).join("debug/wisp-test-app.wasm");
    let shims = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/wisp-cli/src/targets");
    let out = Temp::new("parity");
    std::fs::create_dir_all(&*out).unwrap();
    let put = |from: PathBuf, to: &str| std::fs::copy(from, out.join(to)).expect(to);
    put(shims.join("node.mjs"), "server.mjs");
    put(shims.join("bridge.js"), "bridge.mjs");
    put(wasm, "app.wasm");
    let server = out.join("server.mjs");
    Some((out, server))
}

/// A port nobody has, for Node to listen on.
fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

#[test]
fn node_answers_as_the_native_server_does() {
    let Some((_dir, server)) = node_app() else {
        return;
    };
    let native = common::start(&[]);
    let port = free_port();
    let mut child = Command::new("node")
        .arg(&server)
        .env("PORT", port.to_string())
        .env("HOST", "127.0.0.1")
        .env("WISP_SECRET", SECRET)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("start node");
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let node = Server { child, port };
    assert!(line.contains("listening"), "node said {line:?}");
    for (method, target, headers, body) in REQUESTS {
        let a = essence(&native.request(method, target, headers, body.as_bytes()));
        let b = essence(&node.request(method, target, headers, body.as_bytes()));
        assert_eq!(b, a, "{method} {target}");
    }
}
