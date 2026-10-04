//! The app built for the edge (`--target node`: `app.wasm` and the shims
//! `wisp build` ships) answers a fixed set of requests as the native server
//! does: the same status, content type, redirect and body. Skipped, with a
//! note, when Node or Rust's wasm32 target is not there.

mod common;

use common::{SECRET, Server, Temp, header, status};
use std::io::{BufRead, BufReader, Read, Write};
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
/// node` (or `bun`, or `deno`) would write, without the CLI: `None` where it
/// cannot run. The server file is in the folder as `server`.
fn edge_app(runtime: &str) -> Option<(Temp, PathBuf)> {
    if !have(Command::new(runtime).arg("--version")) {
        eprintln!("parity: no {runtime}, skipped");
        return None;
    }
    let (shim, server) = match runtime {
        "deno" => ("deno.ts", "server.ts"),
        "bun" => ("bun.mjs", "server.mjs"),
        _ => ("node.mjs", "server.mjs"),
    };
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
    put(shims.join(shim), server);
    put(shims.join("bridge.js"), "bridge.mjs");
    put(wasm, "app.wasm");
    let server = out.join(server);
    Some((out, server))
}

/// A port nobody has, for Node to listen on.
fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

/// Node serving the app, over raw sockets (what it does when its check
/// passes) or, with `WISP_NODE_HTTP=1`, over node:http. `None` where it cannot
/// run. The folder lives as long as the server.
fn node(env: &[(&str, &str)]) -> Option<(Server, Temp)> {
    host("node", env)
}

/// [`node`] for `runtime`: Node, Bun or Deno, whichever is installed.
fn host(runtime: &str, env: &[(&str, &str)]) -> Option<(Server, Temp)> {
    let (dir, server) = edge_app(runtime)?;
    let port = free_port();
    let mut cmd = Command::new(runtime);
    if runtime == "deno" {
        cmd.args(["run", "-A"]);
    }
    let mut child = cmd
        .arg(&server)
        .env("PORT", port.to_string())
        .env("HOST", "127.0.0.1")
        .env("WISP_SECRET", SECRET)
        .envs(env.iter().copied())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start node");
    let mut line = String::new();
    let mut said = String::new();
    BufReader::new(child.stderr.take().unwrap())
        .read_line(&mut said)
        .unwrap();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let want = if env.is_empty() {
        "raw sockets"
    } else {
        "node:http"
    };
    assert!(said.contains(want), "node said {said:?}");
    assert!(line.contains("listening"), "node said {line:?}");
    Some((Server { child, port }, dir))
}

/// The native server and Node's over raw sockets.
fn both() -> Option<(Server, Server, Temp)> {
    let (node, dir) = node(&[])?;
    Some((common::start(&[]), node, dir))
}

#[test]
fn node_answers_as_the_native_server_does() {
    let Some((native, node, _dir)) = both() else {
        return;
    };
    for (method, target, headers, body) in REQUESTS {
        let a = essence(&native.request(method, target, headers, body.as_bytes()));
        let b = essence(&node.request(method, target, headers, body.as_bytes()));
        assert_eq!(b, a, "{method} {target}");
    }
}

#[test]
fn node_over_http_answers_as_the_native_server_does() {
    let Some((node, _dir)) = node(&[("WISP_NODE_HTTP", "1")]) else {
        return;
    };
    let native = common::start(&[]);
    for (method, target, headers, body) in REQUESTS {
        let a = essence(&native.request(method, target, headers, body.as_bytes()));
        let b = essence(&node.request(method, target, headers, body.as_bytes()));
        assert_eq!(b, a, "{method} {target}");
    }
}

/// Every answer in `raw`, one after another, each reduced as `essence` does.
fn answers(mut raw: &str) -> Vec<String> {
    let mut all = Vec::new();
    while let Some((head, rest)) = raw.split_once("\r\n\r\n") {
        let lower = head.to_ascii_lowercase();
        let end = if lower.contains("transfer-encoding: chunked") {
            rest.find("\r\n0\r\n\r\n").map_or(rest.len(), |i| i + 7)
        } else {
            lower
                .split("content-length: ")
                .nth(1)
                .and_then(|l| l.split("\r\n").next()?.trim().parse().ok())
                .unwrap_or(0)
        };
        let end = end.min(rest.len());
        all.push(essence(&format!("{head}\r\n\r\n{}", &rest[..end])));
        raw = &rest[end..];
    }
    all
}

#[test]
fn pipelined_requests_are_answered_in_order_as_native_does() {
    let Some((native, node, _dir)) = both() else {
        return;
    };
    let wire = |port: u16| {
        let h = format!("host: 127.0.0.1:{port}\r\n");
        format!(
            "GET / HTTP/1.1\r\n{h}\r\nPOST /echo HTTP/1.1\r\n{h}content-length: 5\r\n\r\nhelloGET /styled HTTP/1.1\r\n{h}\r\nGET /nope HTTP/1.1\r\n{h}connection: close\r\n\r\n"
        )
    };
    let a = answers(&native.send(wire(native.port).as_bytes()));
    let b = answers(&node.send(wire(node.port).as_bytes()));
    assert_eq!(a.len(), 4, "{a:?}");
    assert_eq!(b, a);
}

#[test]
fn chunked_bodies_arrive_whole_as_native_has_them() {
    let Some((native, node, _dir)) = both() else {
        return;
    };
    let wire = |port: u16| {
        format!(
            "POST /echo HTTP/1.1\r\nhost: 127.0.0.1:{port}\r\nconnection: close\r\ntransfer-encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n"
        )
    };
    let a = essence(&native.send(wire(native.port).as_bytes()));
    let b = essence(&node.send(wire(node.port).as_bytes()));
    assert!(a.contains("hello world"), "{a}");
    assert_eq!(b, a);
    // A byte at a time, so the bytes stop inside every chunk.
    let mut c = common::connect(node.port);
    for byte in wire(node.port).bytes() {
        c.write_all(&[byte]).unwrap();
        c.flush().unwrap();
    }
    let mut got = String::new();
    c.read_to_string(&mut got).unwrap();
    assert_eq!(essence(&got), a);
}

#[test]
fn a_connection_is_kept_for_the_next_request_as_native_keeps_it() {
    let Some((native, node, _dir)) = both() else {
        return;
    };
    let run = |s: &Server| {
        let mut c = BufReader::new(common::connect(s.port));
        let mut seen = Vec::new();
        for target in ["/", "/styled", "/nope", "/"] {
            let req = format!(
                "GET {target} HTTP/1.1\r\nhost: 127.0.0.1:{}\r\n\r\n",
                s.port
            );
            c.get_mut().write_all(req.as_bytes()).unwrap();
            let (head, body) = common::read_answer(&mut c);
            assert!(
                !head.to_ascii_lowercase().contains("connection: close"),
                "{head}"
            );
            seen.push(format!("{} {}", head.lines().next().unwrap(), body.len()));
        }
        // HTTP/1.0 closes after its answer.
        c.get_mut().write_all(b"GET / HTTP/1.0\r\n\r\n").unwrap();
        let mut rest = String::new();
        c.read_to_string(&mut rest).unwrap();
        seen.push(essence(&rest));
        seen
    };
    assert_eq!(run(&node), run(&native));
}

#[test]
fn a_head_too_large_is_refused_as_native_refuses_it() {
    let Some((native, node, _dir)) = both() else {
        return;
    };
    let wire = |port: u16| {
        format!(
            "GET / HTTP/1.1\r\nhost: 127.0.0.1:{port}\r\nx-big: {}\r\n\r\n",
            "a".repeat(40 * 1024)
        )
    };
    let a = native.send(wire(native.port).as_bytes());
    let b = node.send(wire(node.port).as_bytes());
    assert_eq!(status(&a), 431, "{a}");
    assert_eq!(status(&b), 431, "{b}");
    // Malformed, too.
    let bad = |s: &Server| status(&s.send(b"GET / HTTP/1.1\r\nhost: x\r\nhost: y\r\n\r\n"));
    assert_eq!(bad(&node), bad(&native));
}

/// Bun's and Deno's shims drive the same raw path: where they are installed
/// they answer as the native server does, and skip, with a note, where not.
#[test]
fn bun_and_deno_answer_as_the_native_server_does() {
    let native = common::start(&[]);
    for runtime in ["bun", "deno"] {
        let Some((server, _dir)) = host(runtime, &[]) else {
            continue;
        };
        for (method, target, headers, body) in REQUESTS {
            let a = essence(&native.request(method, target, headers, body.as_bytes()));
            let b = essence(&server.request(method, target, headers, body.as_bytes()));
            assert_eq!(b, a, "{runtime} {method} {target}");
        }
        let wire = |port: u16| {
            let h = format!("host: 127.0.0.1:{port}\r\n");
            format!("GET / HTTP/1.1\r\n{h}\r\nGET /nope HTTP/1.1\r\n{h}connection: close\r\n\r\n")
        };
        let a = answers(&native.send(wire(native.port).as_bytes()));
        let b = answers(&server.send(wire(server.port).as_bytes()));
        assert_eq!(b, a, "{runtime} pipelined");
    }
}

#[test]
fn a_streamed_body_is_chunked_and_the_connection_goes_on() {
    let Some((native, node, _dir)) = both() else {
        return;
    };
    let wire = |port: u16| {
        let h = format!("host: 127.0.0.1:{port}\r\n");
        format!(
            "GET /lines HTTP/1.1\r\n{h}\r\nGET / HTTP/1.1\r\n{h}\r\nGET /lines HTTP/1.1\r\n{h}connection: close\r\n\r\n"
        )
    };
    let a = answers(&native.send(wire(native.port).as_bytes()));
    let b = answers(&node.send(wire(node.port).as_bytes()));
    assert_eq!(a.len(), 3, "{a:?}");
    assert!(a[0].contains('3'), "{a:?}");
    assert_eq!(b, a);
}
