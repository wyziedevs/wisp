//! The built-in server on a real socket, in this process: framing, keep-alive,
//! pipelining, chunked bodies both ways, streamed responses, malformed
//! requests and clients that leave. It serves the hand-written app in `common/`.

mod common;
#[path = "../../../tests/shared/ws.rs"]
mod ws;

use common::Lab;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::OnceLock;
use std::time::Duration;

/// The port of the one server all tests share, started on first use.
fn port() -> u16 {
    static PORT: OnceLock<u16> = OnceLock::new();
    *PORT.get_or_init(|| {
        let free = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let _ = runtime.block_on(wisp::serve::<Lab>(([127, 0, 0, 1], free).into()));
        });
        for _ in 0..600 {
            if TcpStream::connect(("127.0.0.1", free)).is_ok() {
                return free;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the server did not start");
    })
}

/// A connection, and what it has read of the server's answers.
struct Wire(BufReader<TcpStream>);

/// One answer.
struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Answer {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

fn connect() -> Wire {
    let stream = TcpStream::connect(("127.0.0.1", port())).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream.set_nodelay(true).unwrap();
    Wire(BufReader::new(stream))
}

impl Wire {
    fn write(&mut self, bytes: &[u8]) {
        self.0.get_mut().write_all(bytes).unwrap();
    }

    fn line(&mut self) -> String {
        let mut line = String::new();
        self.0.read_line(&mut line).unwrap();
        line.trim_end_matches(['\r', '\n']).to_string()
    }

    /// The status line and headers of the next answer.
    fn head(&mut self) -> Answer {
        let status_line = self.line();
        assert!(status_line.starts_with("HTTP/1."), "{status_line:?}");
        let status = status_line[9..12].parse().unwrap();
        let mut headers = Vec::new();
        loop {
            let line = self.line();
            if line.is_empty() {
                return Answer {
                    status,
                    headers,
                    body: Vec::new(),
                };
            }
            let (name, value) = line.split_once(": ").unwrap();
            headers.push((name.to_string(), value.to_string()));
        }
    }

    /// The next whole answer, its body framed as it says.
    fn answer(&mut self) -> Answer {
        let mut a = self.head();
        if a.status < 200 || a.status == 204 || a.status == 304 {
            return a;
        }
        if a.header("transfer-encoding") == Some("chunked") {
            loop {
                let size = usize::from_str_radix(&self.line(), 16).unwrap();
                let mut chunk = vec![0; size + 2];
                self.0.read_exact(&mut chunk).unwrap();
                assert_eq!(&chunk[size..], b"\r\n");
                if size == 0 {
                    return a;
                }
                a.body.extend_from_slice(&chunk[..size]);
            }
        }
        match a.header("content-length") {
            Some(n) => {
                a.body = vec![0; n.parse().unwrap()];
                self.0.read_exact(&mut a.body).unwrap();
            }
            None => {
                self.0.read_to_end(&mut a.body).unwrap();
            }
        }
        a
    }

    /// The next chunk of a chunked body, until the empty one.
    fn chunk(&mut self) -> Vec<u8> {
        let size = usize::from_str_radix(&self.line(), 16).unwrap();
        let mut chunk = vec![0; size + 2];
        self.0.read_exact(&mut chunk).unwrap();
        chunk.truncate(size);
        chunk
    }

    /// Everything until the server closes the connection.
    fn rest(&mut self) -> Vec<u8> {
        let mut all = Vec::new();
        self.0
            .read_to_end(&mut all)
            .expect("the server should have closed the connection");
        all
    }

    /// Whether the server has closed the connection.
    fn closed(&mut self) -> bool {
        self.rest().is_empty()
    }

    /// One request and its answer.
    fn ask(&mut self, request: &str) -> Answer {
        self.write(request.as_bytes());
        self.answer()
    }
}

fn get(target: &str) -> String {
    format!("GET {target} HTTP/1.1\r\nhost: lab\r\n\r\n")
}

/// The answer to `raw`, sent whole on a new connection that the server closes.
fn once(raw: &[u8]) -> Answer {
    let mut wire = connect();
    wire.write(raw);
    let answer = wire.answer();
    let sent = String::from_utf8_lossy(raw)
        .chars()
        .take(80)
        .collect::<String>();
    assert_eq!(answer.header("connection"), Some("close"), "{sent:?}");
    assert!(wire.closed(), "{sent:?}");
    answer
}

#[test]
fn keep_alive_serves_many_requests_on_one_connection() {
    let mut wire = connect();
    for i in 0..50 {
        let a = wire.ask(&get(&format!("/q?s={i}")));
        assert_eq!(a.status, 200);
        assert_eq!(a.text(), format!("s={i}|5|{i}"));
        assert_eq!(a.header("connection"), None, "kept open by default");
        assert_eq!(
            a.header("content-length"),
            Some(a.body.len().to_string().as_str())
        );
    }
    let closing = wire.ask("GET /hello HTTP/1.1\r\nhost: lab\r\nconnection: close\r\n\r\n");
    assert_eq!(closing.status, 200);
    assert_eq!(closing.header("connection"), Some("close"));
    assert!(wire.closed());
}

#[test]
fn pipelined_requests_are_answered_in_order() {
    let mut wire = connect();
    let mut all = String::new();
    for i in 0..5 {
        all.push_str(&get(&format!("/q?s={i}")));
    }
    wire.write(all.as_bytes());
    for i in 0..5 {
        assert_eq!(wire.answer().text(), format!("s={i}|5|{i}"));
    }
    // A request that closes the connection ends it: what was pipelined after it is not answered.
    wire.write(
        b"GET /hello HTTP/1.1\r\nconnection: close\r\nhost: lab\r\n\r\nGET /q?s=late HTTP/1.1\r\nhost: lab\r\n\r\n",
    );
    assert_eq!(wire.answer().status, 200);
    assert!(wire.closed());
}

#[test]
fn a_request_may_arrive_in_pieces() {
    let mut wire = connect();
    let request = b"POST /echo HTTP/1.1\r\nhost: lab\r\ncontent-length: 11\r\n\r\nhello world";
    // A byte at a time: the head and the body are each read as they come.
    for byte in request {
        wire.write(&[*byte]);
    }
    assert_eq!(wire.answer().text(), "11:hello world");
    // Two requests, split mid-way between them.
    let two = format!("{}{}", get("/q?s=1"), get("/q?s=2"));
    let (a, b) = two.as_bytes().split_at(two.len() - 9);
    wire.write(a);
    assert_eq!(wire.answer().text(), "s=1|5|1");
    wire.write(b);
    assert_eq!(wire.answer().text(), "s=2|5|2");
}

#[test]
fn http_10_closes_unless_asked_to_stay() {
    let a = once(b"GET /hello HTTP/1.0\r\n\r\n");
    assert_eq!(a.status, 200);
    let mut wire = connect();
    wire.write(b"GET /q?s=1 HTTP/1.0\r\nconnection: keep-alive\r\n\r\n");
    let kept = wire.answer();
    assert_eq!(
        (kept.status, kept.header("connection")),
        (200, Some("keep-alive"))
    );
    assert_eq!(
        wire.ask("GET /q?s=2 HTTP/1.0\r\nconnection: keep-alive\r\n\r\n")
            .text(),
        "s=2|5|2"
    );
    // No chunked bodies before HTTP/1.1.
    let mut wire = connect();
    wire.write(b"POST /echo HTTP/1.0\r\ntransfer-encoding: chunked\r\n\r\n0\r\n\r\n");
    assert_eq!(wire.answer().status, 400);
}

#[test]
fn a_client_that_waits_to_be_told_to_continue_is() {
    let mut wire = connect();
    wire.write(
        b"POST /echo HTTP/1.1\r\nhost: lab\r\nexpect: 100-continue\r\ncontent-length: 5\r\n\r\n",
    );
    let interim = wire.head();
    assert_eq!(interim.status, 100);
    assert!(interim.headers.is_empty());
    wire.write(b"hello");
    assert_eq!(wire.answer().text(), "5:hello");
    // A body that is already on its way needs no invitation, and the answer is still one.
    let both = wire.ask(
        "POST /echo HTTP/1.1\r\nhost: lab\r\nexpect: 100-continue\r\ncontent-length: 2\r\n\r\nhi",
    );
    assert_eq!(both.text(), "2:hi");
    // One the route would refuse is refused at once, before anyone sends it.
    let mut wire = connect();
    wire.write(
        b"POST /item/x HTTP/1.1\r\nhost: lab\r\nexpect: 100-continue\r\ncontent-length: 100\r\n\r\n",
    );
    assert_eq!(wire.head().status, 413);
    // Other expectations are not ours to promise: the request is read as if there were none.
    let plain = connect()
        .ask("POST /echo HTTP/1.1\r\nhost: lab\r\nexpect: 200-ok\r\ncontent-length: 1\r\n\r\nx");
    assert_eq!(plain.text(), "1:x");
}

#[test]
fn head_has_the_length_and_no_body() {
    let mut wire = connect();
    let full = wire.ask(&get("/hello"));
    wire.write(b"HEAD /hello HTTP/1.1\r\nhost: lab\r\n\r\n");
    let head = wire.head();
    assert_eq!(head.status, 200);
    assert_eq!(
        head.header("content-length"),
        Some(full.body.len().to_string().as_str())
    );
    // Nothing of a body follows: the next answer is the next request's.
    assert_eq!(wire.ask(&get("/q?s=z")).text(), "s=z|5|z");
    // 204 has no length and no body, whatever the app made.
    wire.write(b"GET /resp?k=empty HTTP/1.1\r\nhost: lab\r\n\r\n");
    let empty = wire.head();
    assert_eq!((empty.status, empty.header("content-length")), (204, None));
    assert_eq!(wire.ask(&get("/q?s=y")).text(), "s=y|5|y");
}

#[test]
fn the_date_is_an_http_date() {
    let a = wire_get("/hello");
    let date = a.header("date").unwrap();
    let parts: Vec<&str> = date.split(' ').collect();
    assert_eq!(parts.len(), 6, "{date}");
    assert!(parts[0].ends_with(',') && parts[5] == "GMT", "{date}");
    assert_eq!(parts[4].len(), 8, "{date}");
    let year: u32 = parts[3].parse().unwrap();
    assert!(year >= 2025, "{date}");
    assert_eq!(a.header("content-type"), Some("text/html; charset=utf-8"));
}

fn wire_get(target: &str) -> Answer {
    connect().ask(&get(target))
}

#[test]
fn chunked_request_bodies_are_decoded() {
    let post = |body: &str| {
        let mut wire = connect();
        let a = wire.ask(&format!(
            "POST /echo HTTP/1.1\r\nhost: lab\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n{body}"
        ));
        (a.status, a.text())
    };
    assert_eq!(
        post("5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n"),
        (200, "11:hello world".into())
    );
    assert_eq!(post("0\r\n\r\n"), (200, "0:".into()));
    // Hex sizes in either case, extensions and trailers skipped.
    assert_eq!(
        post("A\r\n0123456789\r\n0\r\n\r\n"),
        (200, "10:0123456789".into())
    );
    assert_eq!(
        post("a\r\n0123456789\r\n0\r\n\r\n"),
        (200, "10:0123456789".into())
    );
    assert_eq!(
        post("3;name=value\r\nabc\r\n0;last\r\nx-trailer: 1\r\n\r\n"),
        (200, "3:abc".into())
    );
    for bad in [
        "zz\r\nabc\r\n0\r\n\r\n",
        "3\r\nabcdef\r\n0\r\n\r\n",
        "3\r\nabc0\r\n\r\n",
        "3\nabc\n0\n\n",
        "-3\r\nabc\r\n0\r\n\r\n",
        "\r\n0\r\n\r\n",
        "00000000000000003\r\nabc\r\n0\r\n\r\n",
        "fffffffffffffff\r\n",
    ] {
        let (status, _) = post(bad);
        assert!(matches!(status, 400 | 413), "{bad:?} -> {status}");
    }
    // Both framings, or another coding, is not guessed at: request smuggling.
    let both = once(b"POST /echo HTTP/1.1\r\nhost: lab\r\ntransfer-encoding: chunked\r\ncontent-length: 5\r\n\r\n0\r\n\r\n");
    assert_eq!(both.status, 400);
    let twice = once(b"POST /echo HTTP/1.1\r\nhost: lab\r\ntransfer-encoding: chunked\r\ntransfer-encoding: chunked\r\n\r\n0\r\n\r\n");
    assert_eq!(twice.status, 400);
    let gzip =
        once(b"POST /echo HTTP/1.1\r\nhost: lab\r\ntransfer-encoding: gzip\r\n\r\n0\r\n\r\n");
    assert_eq!(gzip.status, 501);
    let both_codings = once(
        b"POST /echo HTTP/1.1\r\nhost: lab\r\ntransfer-encoding: gzip, chunked\r\n\r\n0\r\n\r\n",
    );
    assert_eq!(both_codings.status, 501);
    // A route's limit counts what is decoded, not the framing.
    let mut wire = connect();
    let over = wire.ask("POST /item/x HTTP/1.1\r\nhost: lab\r\ntransfer-encoding: chunked\r\n\r\n11\r\naaaaaaaaaaaaaaaaa\r\n0\r\n\r\n");
    assert_eq!(over.status, 413);
}

#[test]
fn framing_may_not_dwarf_the_body() {
    let chunked = |body: &str| {
        let mut wire = connect();
        wire.write(b"POST /echo HTTP/1.1\r\nhost: lab\r\ntransfer-encoding: chunked\r\n\r\n");
        wire.write(body.as_bytes());
        wire
    };
    // A byte of data behind seventy of framing: a body made mostly of extensions, far past the
    // slack of one head. The server refuses part way and closes with the rest still coming, which
    // a client may see as the answer or as a reset, but never as a success.
    let padded = format!("1;e={}\r\nx\r\n", "a".repeat(60)).repeat(1000);
    let mut wire = chunked(&format!("{padded}0\r\n\r\n"));
    let mut got = Vec::new();
    if let Err(e) = wire.0.read_to_end(&mut got) {
        assert_eq!(e.kind(), std::io::ErrorKind::ConnectionReset);
    }
    let got = String::from_utf8_lossy(&got);
    assert!(
        got.is_empty() || got.starts_with("HTTP/1.1 400") || got.starts_with("HTTP/1.1 413"),
        "{got}"
    );
    // Ordinary framing of the same data is fine.
    let fair = chunked(&format!("{}0\r\n\r\n", "1\r\nx\r\n".repeat(200))).answer();
    assert_eq!(fair.text().len(), "200:".len() + 200);
}

#[test]
fn streams_are_sent_as_they_are_made() {
    let mut wire = connect();
    let a = wire.ask(&get("/stream"));
    assert_eq!(a.status, 200);
    assert_eq!(a.header("transfer-encoding"), Some("chunked"));
    assert_eq!(a.header("content-length"), None);
    assert_eq!(a.text(), "ab");
    // The connection is still good after the last chunk.
    assert_eq!(wire.ask(&get("/q?s=1")).text(), "s=1|5|1");

    let events = wire.ask(&get("/events"));
    assert_eq!(events.header("content-type"), Some("text/event-stream"));
    assert_eq!(
        events.text(),
        "data: one\n\ndata: two\ndata: lines\ndata: three\n\n"
    );

    // HTTP/1.0 has no chunks: the body ends with the connection.
    let mut old = connect();
    old.write(b"GET /stream HTTP/1.0\r\n\r\n");
    let head = old.head();
    assert_eq!((head.status, head.header("transfer-encoding")), (200, None));
    assert_eq!(old.rest(), b"ab");
    // A HEAD for a stream has its headers, and the stream is never started.
    let mut wire = connect();
    wire.write(b"HEAD /stream HTTP/1.1\r\nhost: lab\r\n\r\n");
    assert_eq!(wire.head().status, 200);
    assert_eq!(wire.ask(&get("/q?s=2")).text(), "s=2|5|2");
}

#[test]
fn a_stream_stops_being_made_once_its_client_has_gone() {
    let ended = || wire_get("/ended").text().parse::<usize>().unwrap();
    let before = ended();
    let mut stream = connect();
    stream.write(get("/forever").as_bytes());
    assert_eq!(stream.head().status, 200);
    for _ in 0..3 {
        assert_eq!(stream.chunk(), b"data: tick\n\n");
    }
    drop(stream);
    // The next `send` fails, so the closure's `?` ends it; nothing keeps it running.
    for _ in 0..400 {
        if ended() > before {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("the stream was still being made two seconds after its client left");
}

#[test]
fn malformed_requests_are_refused_and_the_server_carries_on() {
    let cases: &[(&[u8], u16)] = &[
        (b"GARBAGE\r\n\r\n", 400),
        (b"GET / HTTP/1.1\r\nhost x\r\n\r\n", 400),
        (b"GET / HTTP/1.1\r\nx a: b\r\n\r\n", 400),
        (b"GET / HTTP/1.1\r\n : b\r\n\r\n", 400),
        (b"GET / HTTP/1.1\r\nx: a\x00b\r\n\r\n", 400),
        (b"GET / HTTP/1.1\r\nx: a\r\n folded\r\n\r\n", 400),
        (b"GET  / HTTP/1.1\r\n\r\n", 400),
        (b"GET /\x01 HTTP/1.1\r\n\r\n", 400),
        (b"GET /\xff HTTP/1.1\r\n\r\n", 400),
        (
            b"GET / HTTP/1.1\r\ncontent-length: 1\r\ncontent-length: 2\r\n\r\nab",
            400,
        ),
        (b"GET / HTTP/1.1\r\ncontent-length: -1\r\n\r\n", 400),
        (b"GET / HTTP/1.1\r\ncontent-length: 1 1\r\n\r\n", 400),
        (b"GET / HTTP/1.1\r\ncontent-length: +1\r\n\r\n", 400),
        (b"GET / HTTP/1.1\r\ncontent-length: 0x1\r\n\r\n", 400),
        (
            b"GET / HTTP/1.1\r\ncontent-length: 99999999999999999999\r\n\r\n",
            400,
        ),
        (
            b"POST /echo HTTP/1.1\r\ncontent-length: 99999999\r\n\r\n",
            413,
        ),
    ];
    for (raw, want) in cases {
        let a = once(raw);
        assert_eq!(a.status, *want, "{:?}", String::from_utf8_lossy(raw));
    }
    // Not a path, or not a method: refused, but the request was framed, so the connection stays.
    let mut wire = connect();
    assert_eq!(wire.ask("GET * HTTP/1.1\r\nhost: lab\r\n\r\n").status, 400);
    // The absolute form a proxy sends is its path, on its host (RFC 9112 §3.2.2).
    assert_eq!(
        wire.ask("GET http://example.com/q?s=abs HTTP/1.1\r\nhost: lab\r\n\r\n")
            .text(),
        "s=abs|5|abs"
    );
    assert_eq!(wire.ask(&get("/q?s=still")).text(), "s=still|5|still");
    // Heads that never end, one byte past the 16 KB limit: the server has read all of it when it
    // refuses, so it can close cleanly.
    let endless = |start: &str| format!("{start}{}", "a".repeat(16 * 1024 + 1 - start.len()));
    assert_eq!(
        once(endless("GET / HTTP/1.1\r\nx-big: ").as_bytes()).status,
        431
    );
    assert_eq!(once(endless("GET /").as_bytes()).status, 431);
    let many: String = (0..101).map(|i| format!("x-{i}: 1\r\n")).collect();
    assert_eq!(
        once(format!("GET / HTTP/1.1\r\n{many}\r\n").as_bytes()).status,
        431
    );
    let sixty_four: String = (0..64).map(|i| format!("x-{i}: 1\r\n")).collect();
    assert_eq!(
        wire_head(&format!(
            "GET /hello HTTP/1.1\r\nhost: lab\r\n{sixty_four}\r\n"
        )),
        200
    );
    assert_eq!(wire_get("/q?s=alive").text(), "s=alive|5|alive");
}

fn wire_head(raw: &str) -> u16 {
    connect().ask(raw).status
}

#[test]
fn a_bad_request_between_good_ones_ends_the_connection() {
    let mut wire = connect();
    wire.write(format!("{}GARBAGE\r\n\r\n{}", get("/q?s=1"), get("/q?s=2")).as_bytes());
    assert_eq!(wire.answer().text(), "s=1|5|1");
    assert_eq!(wire.answer().status, 400);
    assert!(
        wire.closed(),
        "what follows garbage cannot be trusted to be a request"
    );
}

#[test]
fn one_handler_failing_does_not_end_the_connection() {
    let mut wire = connect();
    assert_eq!(wire.ask(&get("/err?k=panic")).status, 500);
    assert_eq!(wire.ask(&get("/err?k=io")).status, 500);
    assert_eq!(wire.ask(&get("/q?s=fine")).text(), "s=fine|5|fine");
    // The panic hook does not print a second report for the answer: only the 500.
    assert_eq!(wire.ask(&get("/status?n=1000")).status, 500);
    assert_eq!(wire.ask(&get("/q?s=fine")).status, 200);
}

#[test]
fn large_bodies_go_both_ways() {
    let mut wire = connect();
    let body = vec![b'x'; 1_000_000];
    let mut request = format!(
        "POST /echo HTTP/1.1\r\nhost: lab\r\ncontent-length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    request.extend_from_slice(&body);
    wire.write(&request);
    let a = wire.answer();
    assert_eq!(a.status, 200);
    assert_eq!(a.body.len(), "1000000:".len() + body.len());
    assert!(a.body.starts_with(b"1000000:xxx"));
    // Refused by its length alone, before the body is read (none is sent).
    let refused = wire.ask("POST /echo HTTP/1.1\r\nhost: lab\r\ncontent-length: 1048577\r\n\r\n");
    assert_eq!(refused.status, 413);
    assert!(wire.closed());
}

#[test]
fn clients_that_leave_do_not_hurt_the_server() {
    // Gone mid-head, mid-body, and right after asking for a stream.
    for partial in [
        &b"GET /hel"[..],
        b"POST /echo HTTP/1.1\r\nhost: lab\r\ncontent-length: 100\r\n\r\nabc",
        b"POST /echo HTTP/1.1\r\nhost: lab\r\ntransfer-encoding: chunked\r\n\r\n5\r\nab",
        b"GET /stream HTTP/1.1\r\nhost: lab\r\n\r\n",
        b"",
    ] {
        let mut wire = connect();
        wire.write(partial);
        drop(wire);
    }
    // One that half-closes after its request still gets the answer.
    let mut wire = connect();
    wire.write(get("/q?s=half").as_bytes());
    wire.0
        .get_ref()
        .shutdown(std::net::Shutdown::Write)
        .unwrap();
    assert_eq!(wire.answer().text(), "s=half|5|half");
    assert_eq!(wire_get("/q?s=after").text(), "s=after|5|after");
}

#[test]
fn many_connections_at_once() {
    let mut wires: Vec<Wire> = (0..40).map(|_| connect()).collect();
    for (i, wire) in wires.iter_mut().enumerate() {
        wire.write(get(&format!("/q?s={i}")).as_bytes());
    }
    for (i, wire) in wires.iter_mut().enumerate() {
        assert_eq!(wire.answer().text(), format!("s={i}|5|{i}"));
    }
}

#[test]
fn a_forms_origin_is_checked_on_the_wire() {
    let post = |origin: &str| {
        let extra = if origin.is_empty() {
            String::new()
        } else {
            format!("origin: {origin}\r\n")
        };
        connect()
            .ask(&format!(
                "POST /form HTTP/1.1\r\nhost: lab\r\n{extra}content-length: 0\r\nconnection: close\r\n\r\n"
            ))
            .status
    };
    assert_eq!(post(""), 200);
    assert_eq!(post("http://lab"), 200);
    assert_eq!(post("http://evil.example"), 403);
    assert_eq!(post("null"), 403);
}

/// WebSocket clients: the handshake and frames of RFC 6455, from the other side.
mod websocket {
    use super::*;

    const KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";

    fn upgrade(extra: &str) -> String {
        format!(
            "GET /ws HTTP/1.1\r\nhost: lab\r\nupgrade: websocket\r\nconnection: Upgrade\r\nsec-websocket-version: 13\r\nsec-websocket-key: {KEY}\r\n{extra}\r\n"
        )
    }

    fn open() -> Wire {
        let mut wire = connect();
        wire.write(upgrade("").as_bytes());
        let head = wire.head();
        assert_eq!(head.status, 101);
        assert_eq!(head.header("upgrade"), Some("websocket"));
        assert_eq!(head.header("connection"), Some("upgrade"));
        // The example of RFC 6455 section 1.3.
        assert_eq!(
            head.header("sec-websocket-accept"),
            Some("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")
        );
        wire
    }

    use ws::frame;

    fn read(wire: &mut Wire) -> (u8, Vec<u8>) {
        ws::read(&mut wire.0)
    }

    /// The code the server closed with.
    fn closed_with(wire: &mut Wire) -> u16 {
        let (first, payload) = read(wire);
        assert_eq!(first, 0x88, "a close frame");
        assert!(wire.closed());
        u16::from_be_bytes([payload[0], payload[1]])
    }

    #[test]
    fn messages_are_echoed() {
        let mut wire = open();
        wire.write(&frame(0x81, b"hello"));
        assert_eq!(read(&mut wire), (0x81, b"hello".to_vec()));
        wire.write(&frame(0x82, &[0, 1, 2, 255]));
        assert_eq!(read(&mut wire), (0x82, vec![0, 1, 2, 255]));
        // Empty, and the two longer length forms.
        wire.write(&frame(0x81, b""));
        assert_eq!(read(&mut wire), (0x81, Vec::new()));
        let medium = vec![b'm'; 300];
        wire.write(&frame(0x82, &medium));
        assert_eq!(read(&mut wire), (0x82, medium));
        let large = vec![b'l'; 70_000];
        wire.write(&frame(0x82, &large));
        assert_eq!(read(&mut wire), (0x82, large));
        // A message in fragments, with a ping between them, is one message.
        wire.write(&frame(0x01, b"he"));
        wire.write(&frame(0x89, b"p"));
        wire.write(&frame(0x00, b"l"));
        wire.write(&frame(0x80, b"lo"));
        assert_eq!(read(&mut wire), (0x8a, b"p".to_vec()));
        assert_eq!(read(&mut wire), (0x81, b"hello".to_vec()));
        // Several frames in one write.
        let mut burst = frame(0x81, b"one");
        burst.extend(frame(0x81, b"two"));
        wire.write(&burst);
        assert_eq!(read(&mut wire), (0x81, b"one".to_vec()));
        assert_eq!(read(&mut wire), (0x81, b"two".to_vec()));
        // A pong is not answered, and the close is.
        wire.write(&frame(0x8a, b"x"));
        wire.write(&frame(0x88, &[0x03, 0xe8, b'b', b'y', b'e']));
        let (first, payload) = read(&mut wire);
        assert_eq!((first, payload), (0x88, vec![0x03, 0xe8, b'b', b'y', b'e']));
        assert!(wire.closed());
    }

    #[test]
    fn the_client_breaking_the_protocol_is_closed_on() {
        let cases: &[(&str, Vec<u8>, u16)] = &[
            ("unmasked", vec![0x81, 0x01], 1002),
            ("reserved bit", vec![0xc1, 0x81], 1002),
            ("unknown opcode", vec![0x83, 0x81], 1002),
            ("continuation with nothing before", frame(0x80, b"x"), 1002),
            (
                "data inside a fragmented message",
                [frame(0x01, b"a"), frame(0x81, b"b")].concat(),
                1002,
            ),
            ("fragmented ping", vec![0x09, 0x81], 1002),
            ("long ping", vec![0x89, 0x80 | 126, 0, 126], 1002),
            ("close with one byte", frame(0x88, &[1]), 1002),
            (
                "close with a code that is not sent",
                frame(0x88, &[0x03, 0xed]),
                1002,
            ),
            (
                "close with a reason that is not text",
                frame(0x88, &[0x03, 0xe8, 0xff]),
                1002,
            ),
            ("text that is not UTF-8", frame(0x81, &[0xff, 0xfe]), 1007),
            (
                "text cut inside a character",
                frame(0x81, &[0xe2, 0x82]),
                1007,
            ),
            (
                "a message larger than allowed",
                {
                    let mut f = vec![0x82, 0x80 | 127];
                    f.extend_from_slice(&(1u64 << 40).to_be_bytes());
                    f
                },
                1009,
            ),
            (
                "a length that is not a length",
                {
                    let mut f = vec![0x82, 0x80 | 127];
                    f.extend_from_slice(&u64::MAX.to_be_bytes());
                    f
                },
                1002,
            ),
        ];
        for (what, bytes, code) in cases {
            let mut wire = open();
            wire.write(bytes);
            assert_eq!(closed_with(&mut wire), *code, "{what}");
        }
        // Fragments that add up past the limit, which is the route's body limit.
        let mut wire = open();
        let half = vec![b'a'; 600_000];
        wire.write(&frame(0x02, &half));
        wire.write(&frame(0x00, &half));
        assert_eq!(closed_with(&mut wire), 1009);
    }

    #[test]
    fn a_handshake_that_is_not_one_is_refused() {
        let refuse = |raw: String| {
            let mut wire = connect();
            wire.write(raw.as_bytes());
            wire.answer()
        };
        let plain = refuse("GET /ws HTTP/1.1\r\nhost: lab\r\n\r\n".into());
        assert_eq!(
            (plain.status, plain.header("upgrade")),
            (426, Some("websocket"))
        );
        let post = refuse(upgrade("").replace("GET", "POST"));
        assert_eq!(post.status, 426);
        let old = refuse(upgrade("").replace("HTTP/1.1", "HTTP/1.0"));
        assert_eq!(old.status, 426);
        let version = refuse(upgrade("").replace("version: 13", "version: 8"));
        assert_eq!(version.status, 426);
        assert_eq!(version.header("sec-websocket-version"), Some("13"));
        assert_eq!(refuse(upgrade("").replace("Upgrade", "close")).status, 426);
        assert_eq!(refuse(upgrade("").replace(KEY, "short")).status, 400);
        assert_eq!(
            refuse(upgrade("").replace(&format!("sec-websocket-key: {KEY}\r\n"), "")).status,
            400
        );
        // A page on another site cannot open one with the visitor's cookies.
        assert_eq!(
            refuse(upgrade("origin: https://evil.example\r\n")).status,
            403
        );
        assert_eq!(refuse(upgrade("origin: null\r\n")).status, 403);
        let mut wire = connect();
        wire.write(upgrade("origin: http://lab\r\n").as_bytes());
        assert_eq!(wire.head().status, 101);
        // Tokens of a list, in any case.
        let mut wire = connect();
        wire.write(
            upgrade("")
                .replace("Upgrade", "keep-alive, UPGRADE")
                .as_bytes(),
        );
        assert_eq!(wire.head().status, 101);
    }

    #[test]
    fn a_handler_that_fails_closes_with_a_server_error() {
        let mut wire = connect();
        wire.write(upgrade("").replace("GET /ws", "GET /ws-fail").as_bytes());
        assert_eq!(wire.head().status, 101);
        assert_eq!(read(&mut wire), (0x81, b"before".to_vec()));
        assert_eq!(closed_with(&mut wire), 1011);
    }

    #[test]
    fn a_channel_joins_sockets_and_streams() {
        let join = || {
            let mut wire = connect();
            wire.write(upgrade("").replace("GET /ws", "GET /room").as_bytes());
            assert_eq!(wire.head().status, 101);
            wire
        };
        let (mut a, mut b) = (join(), join());
        // What one says, both hear, the sender too.
        a.write(&frame(0x81, b"hi all"));
        assert_eq!(read(&mut a), (0x81, b"hi all".to_vec()));
        assert_eq!(read(&mut b), (0x81, b"hi all".to_vec()));
        b.write(&frame(0x82, b"binary is not passed on"));
        b.write(&frame(0x81, b"second"));
        assert_eq!(read(&mut a), (0x81, b"second".to_vec()));
        assert_eq!(read(&mut b), (0x81, b"second".to_vec()));

        // An event stream listens to another channel, which a request writes to.
        let mut stream = connect();
        stream.write(get("/live").as_bytes());
        assert_eq!(
            stream.head().header("content-type"),
            Some("text/event-stream")
        );
        assert_eq!(wire_get("/announce?m=news").text(), "1");
        assert_eq!(stream.chunk(), b"data: news\n\n");
    }

    #[test]
    fn the_server_keeps_going_after_sockets_come_and_go() {
        for _ in 0..5 {
            let mut wire = open();
            wire.write(&frame(0x81, b"x"));
            assert_eq!(read(&mut wire).0, 0x81);
            drop(wire);
        }
        assert_eq!(wire_get("/q?s=ok").text(), "s=ok|5|ok");
    }
}
