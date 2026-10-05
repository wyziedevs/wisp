//! The app built for the edge (`--target node`: `app.wasm` and the shims
//! `wisp build` ships) answers a fixed set of requests as the native server
//! does: the same status, content type, redirect and body. Skipped, with a
//! note, when Node or Rust's wasm32 target is not there.

#![cfg(not(target_arch = "wasm32"))]

mod common;
#[path = "../../../tests/shared/ws.rs"]
mod ws;

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
    let want = match (env.iter().any(|(k, _)| *k == "WISP_NODE_HTTP"), runtime) {
        (false, _) => "raw sockets",
        (true, "bun") => "Bun.serve",
        (true, "deno") => "Deno.serve",
        (true, _) => "node:http",
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

/// Drives `bridge.js`'s `fetch` (Workers, Deno, Netlify) with web Requests,
/// one per line of stdin (`method target`, then `name: value` headers joined
/// by `\t`, then the body after `\t\t`), and answers each as `fast status
/// content-type location etag`, the body, and a NUL. `fast`: the Response
/// came back at once (the bodyless path), not as a Promise.
const FETCH: &str = r#"
import { readFileSync } from 'node:fs';
import { wisp } from './bridge.mjs';
// A Windows terminal names it `Path`: the app's `Conf` still finds `PATH`.
const env = { ...process.env };
if (process.platform === 'win32' && env.PATH !== undefined) (env.Path = env.PATH), delete env.PATH;
const app = wisp(new WebAssembly.Module(readFileSync(new URL('./app.wasm', import.meta.url))), env);
let out = '';
for (const line of readFileSync(0, 'utf8').split('\n').filter(Boolean)) {
  const [head, body] = line.split('\t\t');
  const [first, ...hs] = head.split('\t');
  const [method, target] = first.split(' ');
  const headers = hs.map((h) => [h.slice(0, h.indexOf(':')), h.slice(h.indexOf(':') + 1).trim()]);
  const req = new Request('http://127.0.0.1' + target, { method, headers, body: body || undefined, redirect: 'manual' });
  const got = app.fetch(req, '127.0.0.1');
  const fast = !(got instanceof Promise);
  const r = await got;
  const h = (n) => JSON.stringify(r.headers.get(n) ?? '');
  out += `${fast} ${r.status} ${h('content-type')} ${h('location')} ${h('etag')}\n${await r.text()}\0`;
}
process.stdout.write(out);
"#;

/// Requests answered through `bridge.js`'s `fetch`, whose bodyless ones
/// read their headers from the Request only as the app asks for them: the
/// same answers as native, the bodyless ones at once and the others later.
#[test]
fn fetch_answers_as_native_with_headers_read_lazily() {
    let Some((dir, _server)) = edge_app("node") else {
        return;
    };
    std::fs::write(dir.join("fetch.mjs"), FETCH).unwrap();
    let native = common::start(&[]);
    let asset = native.request("GET", "/_app/wisp.js", "", b"");
    let etag = header(&asset, "etag").expect("an etag").to_string();
    let html = "accept: text/html\r\n";
    let big = "x".repeat(64 * 1024 + 1);
    let cases: Vec<(&str, &str, String, &str)> = vec![
        ("GET", "/", String::new(), ""),
        ("GET", "/", String::new(), ""),
        ("HEAD", "/", String::new(), ""),
        ("GET", "/nope", String::new(), ""),
        ("GET", "/nope", html.into(), ""),
        ("GET", "/admin", "cookie: user=forged; a=b\r\n".into(), ""),
        ("GET", "/login/?a=1", html.into(), ""),
        ("GET", "/_app/wisp.js", String::new(), ""),
        (
            "GET",
            "/_app/wisp.js",
            format!("if-none-match: {etag}\r\n"),
            "",
        ),
        ("POST", "/echo", String::new(), "hello"),
        ("POST", "/echo", FORM.into(), "a=1&b=two"),
        // Past the route's BODY_LIMIT: 413 on every host, as native has it.
        ("POST", "/echo", String::new(), &big),
    ];
    let mut stdin = String::new();
    for (method, target, headers, body) in &cases {
        stdin += &format!("{method} {target}");
        for h in headers.split("\r\n").filter(|h| !h.is_empty()) {
            stdin += &format!("\t{h}");
        }
        stdin += &format!("\t\t{body}\n");
    }
    let mut child = Command::new("node")
        .arg(dir.join("fetch.mjs"))
        .env("WISP_SECRET", SECRET)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("start node");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let mut said = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut said)
        .unwrap();
    assert!(child.wait().unwrap().success(), "node failed");
    let answers: Vec<&str> = said.split('\0').filter(|a| !a.is_empty()).collect();
    assert_eq!(answers.len(), cases.len(), "{said}");
    for (i, ((method, target, headers, body), got)) in cases.iter().zip(answers).enumerate() {
        let raw = native.request(method, target, headers, body.as_bytes());
        let h = |n| format!("{:?}", header(&raw, n).unwrap_or(""));
        let text = if *method == "HEAD" {
            ""
        } else {
            common::body(&raw)
        };
        let want = format!(
            "{} {} {} {}\n{text}",
            status(&raw),
            h("content-type"),
            h("location"),
            h("etag")
        );
        if body.len() > 64 * 1024 {
            assert_eq!(status(&raw), 413, "{raw:.60}");
        }
        let (fast, got) = got.split_once(' ').unwrap();
        assert_eq!(got, want, "{method} {target} {headers:?}");
        // The first request starts the instance, so it waits.
        assert_eq!(
            fast == "true",
            i > 0 && body.is_empty() && *method != "POST",
            "{method} {target}"
        );
    }
}

/// A body with no length that never ends, through `bridge.js`'s `fetch`.
const ENDLESS: &str = r#"
import { readFileSync } from 'node:fs';
import { wisp } from './bridge.mjs';
const app = wisp(new WebAssembly.Module(readFileSync(new URL('./app.wasm', import.meta.url))), process.env);
let sent = 0;
const body = new ReadableStream({ pull: (c) => { sent += 4096; c.enqueue(new Uint8Array(4096)); } });
const r = await app.fetch(new Request('http://127.0.0.1/echo', { method: 'POST', body, duplex: 'half' }), '127.0.0.1');
process.stdout.write(`${r.status} ${sent < 1 << 20}`);
"#;

/// A body past the route's limit is not read whole before the 413: a stream
/// that never ends is answered once it passes the limit.
#[test]
fn fetch_stops_reading_a_body_past_the_limit() {
    let Some((dir, _server)) = edge_app("node") else {
        return;
    };
    std::fs::write(dir.join("endless.mjs"), ENDLESS).unwrap();
    let done = Command::new("node")
        .arg(dir.join("endless.mjs"))
        .env("WISP_SECRET", SECRET)
        .output()
        .expect("start node");
    let said = String::from_utf8_lossy(&done.stdout);
    let err = String::from_utf8_lossy(&done.stderr);
    assert_eq!(said, "413 true", "{err}");
}

const UPGRADE: &str = "upgrade: websocket\r\nconnection: Upgrade\r\nsec-websocket-version: 13\r\nsec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\n";

/// The status line and headers of an upgrade's answer, off the wire.
fn read_head(c: &mut std::net::TcpStream) -> String {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut b = [0u8];
        c.read_exact(&mut b).unwrap();
        head.push(b[0]);
    }
    String::from_utf8(head).unwrap()
}

/// What is left of a connection once the server has closed it.
fn ended(c: &mut std::net::TcpStream) -> usize {
    let mut rest = Vec::new();
    c.read_to_end(&mut rest).map(|_| rest.len()).unwrap_or(0)
}

/// What a client sees of `/ws` on `port`, a line for each thing: the same on
/// every host that holds WebSockets.
fn ws_session(port: u16) -> Vec<String> {
    let mut log = Vec::new();
    let hello = || {
        format!(
            "GET /ws HTTP/1.1\r\nhost: 127.0.0.1:{port}\r\norigin: http://127.0.0.1:{port}\r\n{UPGRADE}\r\n"
        )
    };
    // Messages, fragments with a ping between them, binary (one large), a
    // frame in pieces, and a close that is echoed. (A client sends its first
    // frame after the 101: a packet with both is the raw hosts' to read.)
    let mut c = common::connect(port);
    c.write_all(hello().as_bytes()).unwrap();
    let h = read_head(&mut c);
    c.write_all(&ws::frame(0x81, b"first")).unwrap();
    log.push(format!(
        "{} {:?} {:?} {:?}",
        status(&h),
        header(&h, "upgrade"),
        header(&h, "connection").map(str::to_lowercase),
        header(&h, "sec-websocket-accept")
    ));
    log.push(format!("{:?}", ws::read(&mut c)));
    let mut more = ws::frame(0x01, b"frag");
    more.extend(ws::frame(0x89, b"are you there"));
    more.extend(ws::frame(0x80, b"mented"));
    more.extend(ws::frame(0x82, &[0xab; 300]));
    more.extend(ws::frame(0x82, &vec![0xcd; 70_000]));
    c.write_all(&more).unwrap();
    for _ in 0..4 {
        let (op, data) = ws::read(&mut c);
        log.push(format!("{op:#x} {}", data.len()));
    }
    for byte in ws::frame(0x81, b"slowly") {
        c.write_all(&[byte]).unwrap();
    }
    log.push(format!("{:?}", ws::read(&mut c)));
    c.write_all(&ws::frame(0x88, &1000u16.to_be_bytes()))
        .unwrap();
    log.push(format!("{:?} {}", ws::read(&mut c), ended(&mut c)));
    // Broken: unmasked, not UTF-8, a message past the route's limit, an
    // unknown opcode.
    let mut big = vec![0x82, 0xff];
    big.extend((2u64 << 20).to_be_bytes());
    big.extend([1, 2, 3, 4]);
    for bad in [
        vec![0x81, 0x02, b'h', b'i'],
        ws::frame(0x81, &[0xff, 0xfe]),
        big,
        ws::frame(0x03, b""),
    ] {
        let mut c = common::connect(port);
        c.write_all(hello().as_bytes()).unwrap();
        read_head(&mut c);
        c.write_all(&bad).unwrap();
        log.push(format!("{:?} {}", ws::read(&mut c), ended(&mut c)));
    }
    log
}

/// What is refused, as every host refuses it: another site's page, a plain
/// request, another version.
fn ws_refusals(port: u16) -> Vec<String> {
    let refused = |headers: &str| {
        let mut c = common::connect(port);
        let raw = format!(
            "GET /ws HTTP/1.1\r\nhost: 127.0.0.1:{port}\r\nconnection: close\r\n{headers}\r\n"
        );
        c.write_all(raw.as_bytes()).unwrap();
        let mut got = String::new();
        let _ = c.read_to_string(&mut got);
        format!("{} {:?}", status(&got), header(&got, "upgrade"))
    };
    vec![
        refused(&format!("origin: https://evil.example\r\n{UPGRADE}")),
        refused(""),
        refused(&UPGRADE.replace("version: 13", "version: 8")),
    ]
}

/// What a client sees where the host's own socket frames messages (Bun.serve,
/// Deno.serve; Workers is the same): echoes, and a message past the limit,
/// which closes the socket with 1009, or with 1000 where the host will not
/// send that.
fn ws_messages(port: u16) -> Vec<String> {
    let mut log = Vec::new();
    let mut c = common::connect(port);
    let hello = format!(
        "GET /ws HTTP/1.1\r\nhost: 127.0.0.1:{port}\r\norigin: http://127.0.0.1:{port}\r\n{UPGRADE}\r\n"
    );
    c.write_all(hello.as_bytes()).unwrap();
    log.push(status(&read_head(&mut c)).to_string());
    for (op, payload) in [
        (0x81, vec![b'a'; 5]),
        (0x82, vec![0xab; 300]),
        (0x82, vec![0xcd; 70_000]),
    ] {
        c.write_all(&ws::frame(op, &payload)).unwrap();
        let (got, data) = ws::read(&mut c);
        log.push(format!("{got:#x} {}", data.len()));
    }
    for byte in ws::frame(0x81, b"slowly") {
        c.write_all(&[byte]).unwrap();
    }
    log.push(format!("{:?}", ws::read(&mut c)));
    c.write_all(&ws::frame(0x82, &vec![0; 2 << 20])).unwrap();
    let (op, data) = ws::read(&mut c);
    let code = u16::from_be_bytes([data[0], data[1]]);
    log.push(format!("{op:#x} {}", [1009, 1000].contains(&code)));
    log
}

/// A WebSocket on raw sockets (and over node:http's upgrade) behaves as the
/// native server's: handshake, echo, fragments, ping, close, and the
/// protocol errors and the size limit that close it.
#[test]
fn websockets_behave_as_native_on_every_raw_host() {
    let native = common::start(&[]);
    let want = ws_session(native.port);
    assert!(want[0].starts_with("101 "), "{want:?}");
    let too_big = 1009u16.to_be_bytes().to_vec();
    assert!(
        want.iter().any(|l| l.contains(&format!("{too_big:?}"))),
        "1009: {want:?}"
    );
    let http = &[("WISP_NODE_HTTP", "1")][..];
    let refusals = ws_refusals(native.port);
    assert_eq!(refusals[0].split(' ').next(), Some("403"), "{refusals:?}");
    for (runtime, env) in [
        ("node", &[][..]),
        ("node", http),
        ("bun", &[][..]),
        ("deno", &[][..]),
    ] {
        let Some((server, _dir)) = host(runtime, env) else {
            continue;
        };
        eprintln!("parity: websockets on {runtime} {env:?}");
        assert_eq!(ws_session(server.port), want, "{runtime} {env:?}");
        assert_eq!(ws_refusals(server.port), refusals, "{runtime} {env:?}");
    }
    // Where the host frames the messages: echoes, then 1009 (the native
    // server is not asked: it closes without reading the 2 MB, and a client
    // that is still writing may see the connection reset first).
    let messages = [
        "101",
        "0x81 5",
        "0x82 300",
        "0x82 70000",
        "(129, [115, 108, 111, 119, 108, 121])",
        "0x88 true",
    ];
    for runtime in ["bun", "deno"] {
        let Some((server, _dir)) = host(runtime, http) else {
            continue;
        };
        eprintln!("parity: websockets on {runtime} {http:?}");
        assert_eq!(ws_messages(server.port), messages, "{runtime} {http:?}");
        assert_eq!(ws_refusals(server.port), refusals, "{runtime} {http:?}");
    }
}

/// A quiet client is pinged halfway through `WISP_WS_IDLE` and closed with
/// 1001 at the end of it, on the host's timers.
#[test]
fn a_quiet_websocket_is_pinged_then_closed_as_native_does() {
    let Some((server, _dir)) = host("node", &[("WISP_WS_IDLE", "2")]) else {
        return;
    };
    let mut c = common::connect(server.port);
    c.write_all(format!("GET /ws HTTP/1.1\r\nhost: x\r\n{UPGRADE}\r\n").as_bytes())
        .unwrap();
    read_head(&mut c);
    let t = std::time::Instant::now();
    assert_eq!(ws::read(&mut c), (0x89, Vec::new()));
    c.write_all(&ws::frame(0x8a, b"")).unwrap();
    assert_eq!(ws::read(&mut c), (0x89, Vec::new()));
    assert_eq!(ws::read(&mut c), (0x88, 1001u16.to_be_bytes().to_vec()));
    let waited = t.elapsed().as_secs_f64();
    assert!((2.5..3.8).contains(&waited), "{waited}"); // ping at 1, pong, ping at 2, close at 3
}

/// Drives `bridge.js`'s `fetch` with WebSocket requests and a host that makes
/// the socket (Workers' pair, Deno's `upgradeWebSocket`): a stand-in takes
/// messages in and out, as those do.
const SOCKET: &str = r#"
import { readFileSync } from 'node:fs';
import { wisp } from './bridge.mjs';
const module = new WebAssembly.Module(readFileSync(new URL('./app.wasm', import.meta.url)));
class Sock extends EventTarget {
  readyState = 1;
  sent = [];
  send(d) { this.sent.push(typeof d === 'string' ? d : [...d]); }
  close(code, reason) { this.closed = [code, reason]; this.readyState = 3; queueMicrotask(() => this.dispatchEvent(new Event('close'))); }
  say(data) { const e = new Event('message'); e.data = data; this.dispatchEvent(e); }
}
const headers = { host: '127.0.0.1', upgrade: 'websocket', connection: 'Upgrade', 'sec-websocket-version': '13', 'sec-websocket-key': 'dGhlIHNhbXBsZSBub25jZQ==' };
const ask = (app, extra = {}) => app.fetch(new Request('http://127.0.0.1/ws', { headers: { ...headers, ...extra } }), '127.0.0.1');
const tick = () => new Promise((r) => setTimeout(r, 20));
const out = [];
let ws;
const app = wisp(module, process.env, undefined, () => ((ws = new Sock()), { ws, response: new Response('upgraded') }));
// The first request is slow (the instance starts), the second is not.
for (const round of [1, 2]) {
  const res = await ask(app);
  out.push(`${round} ${await res.text()}`);
  ws.say('hi');
  ws.say(new Uint8Array([1, 2, 3]).buffer);
  ws.say('x'.repeat(2 << 20));
  await tick();
  out.push(JSON.stringify([ws.sent, ws.closed]));
}
// The client leaves: the handler ends, and nothing more is sent.
const res = await ask(app);
await res.text();
const bye = new Event('close');
bye.code = 4001;
bye.reason = 'bye';
ws.dispatchEvent(bye);
await tick();
out.push(JSON.stringify(ws.sent));
// Workers keeps the request open until the server end answers the client's close.
const answered = JSON.stringify(ws.closed);
// Not an upgrade, and another site's page.
out.push(`${(await app.fetch(new Request('http://127.0.0.1/ws'), '')).status}`);
out.push(`${(await ask(app, { origin: 'https://evil.example' })).status}`);
// A host with no sockets says so.
const bare = wisp(module, process.env);
const no = await ask(bare);
out.push(`${no.status} ${await no.text()}`);
out.push(answered);
process.stdout.write(out.join('\n'));
"#;

#[test]
fn a_host_made_websocket_carries_the_apps_messages() {
    let Some((dir, _server)) = edge_app("node") else {
        return;
    };
    std::fs::write(dir.join("socket.mjs"), SOCKET).unwrap();
    let done = Command::new("node")
        .arg(dir.join("socket.mjs"))
        .env("WISP_SECRET", SECRET)
        .output()
        .expect("start node");
    let said = String::from_utf8_lossy(&done.stdout);
    let err = String::from_utf8_lossy(&done.stderr);
    assert!(done.status.success(), "{said} {err}");
    let echoed = r#"[["hi",[1,2,3]],[1009,""]]"#;
    let lines: Vec<&str> = said.lines().collect();
    assert_eq!(lines[0], "1 upgraded", "{said} {err}");
    assert_eq!(lines[1], echoed, "{said}");
    assert_eq!(lines[2], "2 upgraded", "{said}");
    assert_eq!(lines[3], echoed, "{said}");
    assert_eq!(lines[4], "[]", "{said}");
    assert_eq!(lines[5], "426", "{said}");
    assert_eq!(lines[6], "403", "{said}");
    assert!(
        lines[7].starts_with("501 WebSockets need a host with sockets"),
        "{said}"
    );
    assert_eq!(lines[8], r#"[4001,"bye"]"#, "{said}");
}

/// What a client sends right behind its handshake is read, on raw sockets and
/// over node:http's upgrade (whose `head` it is).
#[test]
fn a_frame_sent_with_the_handshake_is_read_on_node() {
    for env in [&[][..], &[("WISP_NODE_HTTP", "1")][..]] {
        let Some((server, _dir)) = host("node", env) else {
            return;
        };
        let mut c = common::connect(server.port);
        let mut raw = format!(
            "GET /ws HTTP/1.1\r\nhost: 127.0.0.1:{}\r\n{UPGRADE}\r\n",
            server.port
        )
        .into_bytes();
        raw.extend(ws::frame(0x81, b"first"));
        c.write_all(&raw).unwrap();
        assert_eq!(status(&read_head(&mut c)), 101, "{env:?}");
        assert_eq!(ws::read(&mut c), (0x81, b"first".to_vec()), "{env:?}");
    }
}
