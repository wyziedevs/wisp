//! Wisp runs on Linux, so what only Linux does is checked here, on the test
//! app's binary: stopping on SIGTERM and a second signal, sockets and file
//! descriptors, a restart on the same port, a client that vanishes, and
//! the two ways it takes connections (io_uring, and epoll).
//!
//! Other systems compile this file to nothing; the Linux run of the
//! workspace's tests (`scratchpad/linux-test.sh`) is where it counts.
#![cfg(target_os = "linux")]

mod common;

use common::{Server, connect, spawn};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

fn start() -> Server {
    common::start(&[])
}

/// The server's two ways of taking connections: io_uring (the default,
/// where the kernel has it) and an epoll per worker.
const BACKENDS: [&[(&str, &str)]; 2] = [&[], &[("WISP_IO", "epoll")]];

/// The app started with `env`, and the line it says on stderr about its
/// backend: which way it takes connections, or why it stopped. Its port is
/// 0 when it printed none.
fn start_saying(env: &[(&str, &str)]) -> (Server, String) {
    let mut child = common::command(env)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut said = String::new();
    let mut err = BufReader::new(child.stderr.take().unwrap());
    while !said.starts_with("wisp: io: ") && !said.starts_with("wisp: WISP_IO") {
        said.clear();
        if err.read_line(&mut said).unwrap() == 0 {
            break;
        }
    }
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let port = line.trim().rsplit(':').next().and_then(|p| p.parse().ok());
    (
        Server {
            child,
            port: port.unwrap_or(0),
        },
        said,
    )
}

/// Whether the server has an io_uring open.
fn on_uring(s: &Server) -> bool {
    std::fs::read_dir(format!("/proc/{}/fd", s.child.id()))
        .unwrap()
        .flatten()
        .any(|f| {
            std::fs::read_link(f.path()).is_ok_and(|l| l.to_string_lossy().contains("io_uring"))
        })
}

/// `answer` with its dates blanked: two servers' clocks tick apart.
fn undated(answer: String) -> String {
    answer
        .split("\r\n")
        .map(|l| {
            if l.len() == 35 && l.starts_with("date: ") {
                "date: -"
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\r\n")
}

#[test]
fn each_backend_serves_and_says_which_it_is() {
    // The default takes what its start-up check found working, and says so.
    let (default, said) = start_saying(&[]);
    assert!(said.starts_with("wisp: io: "), "{said}");
    assert_eq!(
        said.starts_with("wisp: io: io_uring"),
        on_uring(&default),
        "{said}"
    );
    let (epoll, said) = start_saying(BACKENDS[1]);
    assert_eq!(said.trim(), "wisp: io: epoll (asked by WISP_IO)");
    assert!(!on_uring(&epoll));
    for s in [&default, &epoll] {
        let mut c = BufReader::new(connect(s.port));
        c.get_mut().write_all(GET).unwrap();
        assert!(read_answer(&mut c).0.starts_with("HTTP/1.1 200"));
    }
    // Asked for io_uring: served on it, or stopped saying why not.
    let (mut uring, said) = start_saying(&[("WISP_IO", "uring")]);
    if said.starts_with("wisp: io: io_uring") {
        assert!(on_uring(&uring));
        let mut c = BufReader::new(connect(uring.port));
        c.get_mut().write_all(GET).unwrap();
        assert!(read_answer(&mut c).0.starts_with("HTTP/1.1 200"));
    } else {
        assert!(
            said.contains("WISP_IO is uring, but io_uring does not work here: "),
            "{said}"
        );
        assert_eq!(uring.child.wait().unwrap().code(), Some(1));
    }
    // A value it does not know stops it, saying why.
    let out = common::command(&[("WISP_IO", "kqueue")])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("WISP_IO is \"kqueue\", which is not epoll or uring"),
        "{said}"
    );
}

#[test]
fn both_backends_send_the_same_bytes() {
    let servers = BACKENDS.map(common::start);
    let requests: [&[u8]; 11] = [
        b"GET / HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n",
        b"HEAD / HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n",
        b"GET /nope HTTP/1.1\r\nhost: x\r\naccept: text/html\r\nconnection: close\r\n\r\n",
        b"GET /files/Card.wisp HTTP/1.0\r\n\r\n",
        b"GET /lines HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n",
        b"GET /events HTTP/1.1\r\nhost: x\r\n\r\nGET / HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n",
        b"POST /echo HTTP/1.1\r\nhost: x\r\ncontent-length: 5\r\n\r\nhelloPOST /echo HTTP/1.1\r\nhost: x\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n3\r\nabc\r\n0\r\n\r\n",
        b"POST /echo HTTP/1.1\r\nhost: x\r\nexpect: 100-continue\r\ncontent-length: 2\r\nconnection: close\r\n\r\nhi",
        b"GET /\x01 HTTP/1.1\r\n\r\n",
        b"POST /echo HTTP/1.1\r\nhost: x\r\ncontent-length: 70000\r\n\r\n",
        b"GET / HTTP/1.1\r\nhost: x\r\n\r\nGET /nope HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n",
    ];
    for raw in requests {
        let [a, b] = servers.each_ref().map(|s| undated(s.send(raw)));
        assert!(!a.is_empty(), "{:?}", String::from_utf8_lossy(raw));
        assert_eq!(a, b, "{:?}", String::from_utf8_lossy(raw));
    }
}

#[test]
fn pipelined_requests_come_back_in_order_and_the_connection_stays() {
    for env in BACKENDS {
        let s = common::start(env);
        let mut raw = Vec::new();
        for i in 0..300 {
            let n = i.to_string();
            raw.extend_from_slice(
                format!(
                    "POST /echo HTTP/1.1\r\nhost: x\r\ncontent-length: {}\r\n\r\n{n}",
                    n.len()
                )
                .as_bytes(),
            );
        }
        let mut c = BufReader::new(connect(s.port));
        c.get_mut().write_all(&raw).unwrap();
        for i in 0..300 {
            let (head, body) = read_answer(&mut c);
            assert!(head.starts_with("HTTP/1.1 200"), "{head}");
            assert_eq!(body, format!("{}:{i}", i.to_string().len()), "{env:?}");
        }
        // Kept alive: a quiet moment, then more.
        std::thread::sleep(Duration::from_millis(50));
        for _ in 0..3 {
            c.get_mut().write_all(GET).unwrap();
            assert!(read_answer(&mut c).0.starts_with("HTTP/1.1 200"));
        }
    }
}

#[test]
fn megabytes_of_answers_reach_a_client_that_reads_slowly() {
    // Pipelined echoes sent while the client does not read yet: the server's
    // sends fill the socket and go out in parts, and it stops receiving
    // until it has answered what it has.
    for env in BACKENDS {
        let s = common::start(env);
        let (n, body) = (40, vec![b'x'; 60_000]);
        let c = connect(s.port);
        let mut w = c.try_clone().unwrap();
        let writer = std::thread::spawn(move || {
            for _ in 0..n {
                let head = format!(
                    "POST /echo HTTP/1.1\r\nhost: x\r\ncontent-length: {}\r\n\r\n",
                    body.len()
                );
                w.write_all(head.as_bytes()).unwrap();
                w.write_all(&body).unwrap();
            }
        });
        std::thread::sleep(Duration::from_millis(300));
        let mut c = BufReader::new(c);
        for i in 0..n {
            let (head, got) = read_answer(&mut c);
            assert!(
                head.starts_with("HTTP/1.1 200"),
                "{env:?} answer {i}: {head}"
            );
            assert_eq!(got.len(), "60000:".len() + 60_000);
            std::thread::sleep(Duration::from_millis(5));
        }
        writer.join().unwrap();
    }
}

#[test]
fn a_request_sent_a_byte_at_a_time() {
    for env in BACKENDS {
        let s = common::start(env);
        let mut c = BufReader::new(connect(s.port));
        c.get_ref().set_nodelay(true).unwrap();
        for b in b"POST /echo HTTP/1.1\r\nhost: x\r\ncontent-length: 5\r\n\r\nhello" {
            c.get_mut().write_all(&[*b]).unwrap();
            std::thread::sleep(Duration::from_millis(2));
        }
        let (head, body) = read_answer(&mut c);
        assert!(head.starts_with("HTTP/1.1 200"), "{env:?} {head}");
        assert_eq!(body, "5:hello");
    }
}

#[test]
fn a_large_upload() {
    for env in BACKENDS {
        let s = common::start(env);
        let photo = vec![7u8; 3 * 1024 * 1024 + 17];
        let mut b = b"--XX\r\ncontent-disposition: form-data; name=\"title\"\r\n\r\nCat\r\n--XX\r\ncontent-disposition: form-data; name=\"photo\"; filename=\"cat.png\"\r\ncontent-type: image/png\r\n\r\n".to_vec();
        b.extend_from_slice(&photo);
        b.extend_from_slice(b"\r\n--XX--\r\n");
        let r = s.request(
            "POST",
            "/upload",
            "content-type: multipart/form-data; boundary=XX\r\n",
            &b,
        );
        let saved = format!("cat.png: {} bytes of image/png", photo.len());
        assert!(r.contains(&saved), "{env:?} {}", &r[..r.len().min(300)]);
    }
}

#[test]
fn hundreds_of_connections_at_once() {
    for env in BACKENDS {
        let s = common::start(env);
        let mut conns: Vec<_> = (0..400).map(|_| BufReader::new(connect(s.port))).collect();
        for _ in 0..3 {
            for c in &mut conns {
                c.get_mut().write_all(GET).unwrap();
            }
            for c in &mut conns {
                assert!(read_answer(c).0.starts_with("HTTP/1.1 200"), "{env:?}");
            }
        }
    }
}

/// The app on `port`, with a limit on its open files when given one
/// (`ulimit -n`, which the shell sets and `exec` keeps).
fn start_at(port: &str, files: Option<u32>) -> Server {
    spawn(command_at(port, files))
}

fn command_at(port: &str, files: Option<u32>) -> Command {
    let exe = env!("CARGO_BIN_EXE_wisp-test-app");
    let mut cmd = match files {
        None => Command::new(exe),
        Some(n) => {
            let mut sh = Command::new("sh");
            sh.args(["-c", &format!("ulimit -n {n} && exec \"$0\""), exe]);
            sh
        }
    };
    common::configure(&mut cmd, &[("PORT", port)]);
    cmd
}

/// A signal to the server, sent as an operator or systemd would.
fn signal(server: &Server, name: &str) {
    let sent = Command::new("sh")
        .args(["-c", &format!("kill -{name} {}", server.child.id())])
        .status()
        .unwrap();
    assert!(sent.success(), "kill -{name}");
}

/// The exit of `child`, which must come within `seconds`.
fn exits_within(child: &mut Child, seconds: u64) -> ExitStatus {
    let until = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < until {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the server was still running {seconds} s after it was told to stop");
}

/// Waits for the server to stop taking connections.
fn refuses_soon(port: u16) {
    for _ in 0..600 {
        if TcpStream::connect(("127.0.0.1", port)).is_err() {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("the server was still taking connections after it was told to stop");
}

/// Waits until the server has read what `client` sent: its socket's receive
/// queue, which `/proc/net/tcp` shows, is empty. On the loopback a write is in
/// that queue by the time it returns, so this is the moment the request is
/// under way, with no guess at how long that takes.
fn wait_until_read(server_port: u16, client: &TcpStream) {
    let client_port = client.local_addr().unwrap().port();
    for _ in 0..1000 {
        let table = std::fs::read_to_string("/proc/net/tcp").unwrap();
        let queued = table.lines().skip(1).find_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            let port = |addr: &str| u16::from_str_radix(addr.rsplit(':').next()?, 16).ok();
            let (local, remote) = (port(f.get(1)?)?, port(f.get(2)?)?);
            (local == server_port && remote == client_port && f.get(3) == Some(&"01"))
                .then(|| usize::from_str_radix(f.get(4)?.split(':').nth(1)?, 16).ok())?
        });
        if queued == Some(0) {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("the server did not read the request");
}

/// The status line and headers of the next answer, without the blank line.
fn read_head(c: &mut BufReader<TcpStream>) -> String {
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
fn read_answer(c: &mut BufReader<TcpStream>) -> (String, String) {
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

const GET: &[u8] = b"GET / HTTP/1.1\r\nhost: x\r\n\r\n";

#[test]
fn sigterm_answers_the_request_under_way_and_exits_cleanly() {
    for env in BACKENDS {
        let mut s = common::start(env);
        let mut c = connect(s.port);
        c.write_all(b"POST /echo HTTP/1.1\r\nhost: x\r\ncontent-length: 4\r\n\r\nab")
            .unwrap();
        wait_until_read(s.port, &c);
        signal(&s, "TERM");
        // No new connection, but the one with a request under way finishes it, and is told to close.
        refuses_soon(s.port);
        c.write_all(b"cd").unwrap();
        let mut answer = String::new();
        c.read_to_string(&mut answer).unwrap();
        assert!(answer.starts_with("HTTP/1.1 200"), "{env:?} {answer}");
        assert!(answer.contains("connection: close"), "{answer}");
        assert!(answer.ends_with("4:abcd"), "{answer}");
        assert!(exits_within(&mut s.child, 5).success());
    }
}

#[test]
fn sigint_stops_it_the_same_way() {
    let mut s = start();
    let mut c = BufReader::new(connect(s.port));
    c.get_mut().write_all(GET).unwrap();
    assert!(read_answer(&mut c).0.starts_with("HTTP/1.1 200"));
    signal(&s, "INT");
    assert!(exits_within(&mut s.child, 5).success());
}

#[test]
fn an_idle_connection_does_not_hold_up_the_exit() {
    let mut s = start();
    let mut c = BufReader::new(connect(s.port));
    c.get_mut().write_all(GET).unwrap();
    assert!(read_answer(&mut c).0.starts_with("HTTP/1.1 200"));
    // Kept alive, and quiet: nothing to wait for.
    signal(&s, "TERM");
    assert!(exits_within(&mut s.child, 5).success());
    // The client sees it end, as it would when a proxy closes an idle connection.
    let mut rest = Vec::new();
    if c.read_to_end(&mut rest).is_ok() {
        assert!(rest.is_empty(), "{rest:?}");
    }
}

#[test]
fn a_second_signal_does_not_wait_for_a_request_that_never_ends() {
    let mut s = start();
    let mut c = connect(s.port);
    // A body that is never finished: under way for as long as the connection stays.
    c.write_all(b"POST /echo HTTP/1.1\r\nhost: x\r\ncontent-length: 4\r\n\r\nab")
        .unwrap();
    wait_until_read(s.port, &c);
    signal(&s, "TERM");
    refuses_soon(s.port);
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        s.child.try_wait().unwrap().is_none(),
        "it waits for the request, up to ten seconds"
    );
    signal(&s, "TERM");
    assert!(exits_within(&mut s.child, 3).success());
}

#[test]
fn streams_end_properly_and_sockets_hear_the_server_is_going_away() {
    for env in BACKENDS {
        streams_end_properly_on(env);
    }
}

fn streams_end_properly_on(env: &[(&str, &str)]) {
    // A stream with no end of its own: it is ended, and its last chunk sent.
    let mut s = common::start(env);
    let mut c = BufReader::new(connect(s.port));
    c.get_mut()
        .write_all(b"GET /t/forever HTTP/1.1\r\nhost: x\r\n\r\n")
        .unwrap();
    let mut line = String::new();
    while line != "\r\n" {
        line.clear();
        c.read_line(&mut line).unwrap();
    }
    line.clear();
    c.read_line(&mut line).unwrap(); // the size of the first chunk
    assert!(usize::from_str_radix(line.trim(), 16).is_ok(), "{line:?}");
    signal(&s, "TERM");
    let mut rest = Vec::new();
    c.read_to_end(&mut rest).unwrap();
    assert!(
        rest.ends_with(b"0\r\n\r\n"),
        "{:?}",
        String::from_utf8_lossy(&rest)
    );
    assert!(exits_within(&mut s.child, 5).success());

    // A WebSocket gets a close frame, code 1001, and then the end.
    let mut s = common::start(env);
    let mut c = BufReader::new(connect(s.port));
    c.get_mut()
        .write_all(
            b"GET /ws HTTP/1.1\r\nhost: x\r\nupgrade: websocket\r\nconnection: Upgrade\r\nsec-websocket-version: 13\r\nsec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
        )
        .unwrap();
    let head = read_head(&mut c);
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    signal(&s, "TERM");
    let mut frame = [0u8; 4];
    c.read_exact(&mut frame).unwrap();
    assert_eq!(frame, [0x88, 0x02, 0x03, 0xe9], "close, 1001 going away");
    let mut rest = Vec::new();
    let _ = c.read_to_end(&mut rest);
    assert!(rest.is_empty());
    assert!(exits_within(&mut s.child, 5).success());
}

#[test]
fn a_restart_takes_the_port_back_at_once() {
    // The server closes these first, which leaves the connections in TIME_WAIT on its side of
    // the port; a listener without SO_REUSEADDR could not bind again until they expire.
    // A port some other test's connection is using cannot be bound, so another is tried.
    for attempt in 0..8 {
        let free = 20000 + ((std::process::id() + attempt * 997) % 10000) as u16;
        let Some(first) = common::try_spawn(command_at(&free.to_string(), None)) else {
            continue;
        };
        for _ in 0..20 {
            let mut c = connect(free);
            c.write_all(b"GET / HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n")
                .unwrap();
            let mut all = Vec::new();
            c.read_to_end(&mut all).unwrap();
            assert!(all.starts_with(b"HTTP/1.1 200"));
        }
        drop(first);
        let Some(second) = common::try_spawn(command_at(&free.to_string(), None)) else {
            continue;
        };
        let mut c = BufReader::new(connect(second.port));
        c.get_mut().write_all(GET).unwrap();
        assert!(read_answer(&mut c).0.starts_with("HTTP/1.1 200"));
        return;
    }
    panic!("no port could be taken back");
}

/// How many files the process has open.
fn open_files(s: &Server) -> usize {
    std::fs::read_dir(format!("/proc/{}/fd", s.child.id()))
        .unwrap()
        .count()
}

#[test]
fn every_way_a_connection_can_end_gives_its_descriptor_back() {
    for env in BACKENDS {
        descriptors_come_back_on(env);
    }
}

fn descriptors_come_back_on(env: &[(&str, &str)]) {
    let s = common::start(env);
    let mut warm = BufReader::new(connect(s.port));
    warm.get_mut().write_all(GET).unwrap();
    read_answer(&mut warm);
    drop(warm);
    let settle = |s: &Server, want: usize| {
        for _ in 0..1000 {
            if open_files(s) <= want {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!(
            "{} files open, {want} before: a connection was not closed",
            open_files(s)
        );
    };
    settle(&s, open_files(&s));
    let before = open_files(&s);

    for i in 0..120 {
        let mut c = connect(s.port);
        match i % 6 {
            // Answered, and closed by the server.
            0 => {
                c.write_all(b"GET / HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n")
                    .unwrap();
                let mut all = Vec::new();
                c.read_to_end(&mut all).unwrap();
            }
            // Answered and left open, then closed by the client.
            1 => {
                let mut c = BufReader::new(c);
                c.get_mut().write_all(GET).unwrap();
                read_answer(&mut c);
            }
            // Gone before a byte.
            2 => {}
            // Gone in the middle of a head, and of a body.
            3 => c.write_all(b"GET /hel").unwrap(),
            4 => c
                .write_all(b"POST /echo HTTP/1.1\r\nhost: x\r\ncontent-length: 50\r\n\r\nabc")
                .unwrap(),
            // Gone while a stream is being made, and while a socket is open.
            _ => {
                let request: &[u8] = if i % 12 == 5 {
                    b"GET /t/forever HTTP/1.1\r\nhost: x\r\n\r\n"
                } else {
                    b"GET /ws HTTP/1.1\r\nhost: x\r\nupgrade: websocket\r\nconnection: Upgrade\r\nsec-websocket-version: 13\r\nsec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
                };
                c.write_all(request).unwrap();
                read_head(&mut BufReader::new(c));
            }
        }
    }
    settle(&s, before);
    // And it still serves.
    let mut c = BufReader::new(connect(s.port));
    c.get_mut().write_all(GET).unwrap();
    assert!(read_answer(&mut c).0.starts_with("HTTP/1.1 200"));
}

#[test]
fn running_out_of_descriptors_pauses_accepting_and_no_more() {
    // A process that may open 48 files: the listeners, the runtimes (and rings) and some thirty
    // connections, fewer on io_uring (a listener and a ring per worker). Of many that come at
    // once, which are accepted is up to the kernel, which spreads them over those listeners.
    let s = start_at("0", Some(48));
    let served = |c: &mut BufReader<TcpStream>| {
        c.get_mut().write_all(GET).unwrap();
        assert!(read_answer(c).0.starts_with("HTTP/1.1 200"));
    };
    let mut first = BufReader::new(connect(s.port));
    served(&mut first);
    // Many more than fit, each listener too.
    let mut conns: Vec<TcpStream> = (0..100).map(|_| connect(s.port)).collect();
    // One that was accepted is served, however many are waiting behind it.
    served(&mut first);
    // One that was not is waiting: the listener has nowhere to put it.
    let mut last = conns.pop().unwrap();
    last.write_all(GET).unwrap();
    last.set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let mut byte = [0u8; 1];
    let waiting = last.read(&mut byte);
    assert!(
        matches!(&waiting, Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)),
        "{waiting:?}"
    );
    // Room again: the waiting ones kept, fewer than fit, are accepted and answered.
    drop(first);
    conns.drain(..conns.len() - 10);
    let mut c = BufReader::new(last);
    c.get_ref()
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    assert!(read_answer(&mut c).0.starts_with("HTTP/1.1 200"));
}

#[test]
fn clients_that_vanish_mid_answer_do_not_kill_the_server() {
    // A write to a socket the peer has reset is EPIPE, and SIGPIPE, which kills a process that
    // has not set it aside. Big answers, and streams, to clients that leave at once.
    let mut s = start();
    let body = vec![b'x'; 60_000];
    for i in 0..60 {
        let mut c = connect(s.port);
        if i % 2 == 0 {
            let head = format!(
                "POST /echo HTTP/1.1\r\nhost: x\r\ncontent-length: {}\r\n\r\n",
                body.len()
            );
            c.write_all(head.as_bytes()).unwrap();
            c.write_all(&body).unwrap();
        } else {
            c.write_all(b"GET /t/forever HTTP/1.1\r\nhost: x\r\n\r\n")
                .unwrap();
        }
        drop(c);
    }
    let mut c = BufReader::new(connect(s.port));
    c.get_mut().write_all(GET).unwrap();
    assert!(read_answer(&mut c).0.starts_with("HTTP/1.1 200"));
    assert!(
        s.child.try_wait().unwrap().is_none(),
        "the server is still running"
    );
}

#[test]
fn a_privileged_port_is_refused_with_what_to_do() {
    // Only where ports below 1024 are for root alone, and this is not root.
    let start_of_unprivileged =
        std::fs::read_to_string("/proc/sys/net/ipv4/ip_unprivileged_port_start")
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(1024);
    let root = std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .any(|l| l.starts_with("Uid:") && l.split_whitespace().nth(1) == Some("0"));
    if root || start_of_unprivileged <= 80 {
        return;
    }
    let out = common::command(&[("PORT", "80")])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("cannot listen on 127.0.0.1:80"), "{said}");
    assert!(said.contains("needs more privileges"), "{said}");
}
