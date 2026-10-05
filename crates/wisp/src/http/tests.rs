use super::*;

#[test]
fn the_unix_second_comes_from_the_tick() {
    let offset = 1_790_000_000;
    assert_eq!(unix_at(offset, 0), offset);
    assert_eq!(unix_at(offset, 12_345), offset + 12_345);
    // The clock set back past the start: the offset wraps, the sum does not.
    assert_eq!(unix_at(5u64.wrapping_sub(10), 10), 5);
}

#[test]
fn static_paths_refuse_windows_devices_and_trimmed_names() {
    for bad in [
        "CON",
        "a/nul.txt",
        "Com1",
        "lpt9.js",
        "aux ",
        "a.txt.",
        "a.txt ",
        "x/PRN",
    ] {
        assert!(!stays_inside(bad), "{bad}");
    }
    for ok in [
        "console.js",
        "a/null.txt",
        "com.css",
        "comx",
        "lpt",
        "a.b.txt",
        "nu",
    ] {
        assert!(stays_inside(ok), "{ok}");
    }
}
#[test]
fn dev_binds_the_next_port_when_one_is_taken() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = taken.local_addr().unwrap();
    let next = bind_near(addr, 20).unwrap().local_addr().unwrap();
    assert_ne!(next.port(), addr.port());
    assert!(next.port() > addr.port() && next.port() <= addr.port() + 20);
    let err = bind_near(addr, 0).unwrap_err();
    assert!(err.to_string().contains("all in use"), "{err}");
}

#[test]
fn dates() {
    assert_eq!(&http_date(784_111_777), b"Sun, 06 Nov 1994 08:49:37 GMT");
    assert_eq!(&http_date(0), b"Thu, 01 Jan 1970 00:00:00 GMT");
    assert_eq!(&http_date(951_782_400), b"Tue, 29 Feb 2000 00:00:00 GMT");
    assert_eq!(&http_date(4_102_444_799), b"Thu, 31 Dec 2099 23:59:59 GMT");
}

#[test]
fn decimals() {
    let mut v = Vec::new();
    for n in [0, 9, 0x10, 0xabc, u64::MAX] {
        push_hex(&mut v, n);
        v.push(b' ');
    }
    assert_eq!(v, b"0 9 10 abc ffffffffffffffff ");
    v.clear();
    push_decimal(&mut v, 0);
    v.push(b' ');
    push_decimal(&mut v, 18_446_744_073_709_551_615);
    assert_eq!(v, b"0 18446744073709551615");
    assert_eq!(parse_decimal(b"1234"), Some(1234));
    assert_eq!(parse_decimal(b""), None);
    assert_eq!(parse_decimal(b"+1"), None);
    assert_eq!(parse_decimal(b"1 "), None);
    assert_eq!(parse_decimal(b"99999999999999999999"), None);
}

#[test]
fn headers_that_would_split_a_response_are_left_out() {
    let mut w = Vec::new();
    let mut put = |n: &'static str, v: &'static str| {
        header(&mut w, &(n.into(), v.into())); // checked, then known
        header(&mut w, &(n.into(), v.to_string().into())); // checked
    };
    put("x-a", "1");
    put("x-b", "2\r\nset-cookie: x=1");
    put("x-c\n", "3");
    put("x-d", "4\0");
    assert_eq!(w, b"x-a: 1\r\nx-a: 1\r\n");
}

#[test]
fn paths() {
    assert_eq!(
        safe_relative_path("/img/a%20b.png").as_deref(),
        Some("img/a b.png")
    );
    for bad in [
        "/../secret",
        "/a/%2e%2e/b",
        "/a//b",
        "/",
        "/C:/x",
        "/a\\b",
        "/a/",
    ] {
        assert_eq!(safe_relative_path(bad), None, "{bad}");
    }
}

use crate::fuzz::{Fuzz, Rng, SMALL, mutate};

/// A request as a client may send it, and what it should parse to.
struct Sent {
    wire: Vec<u8>,
    path: &'static str,
    query: String,
    body: Vec<u8>,
    headers: usize,
}

fn sent(rng: &mut Rng) -> Sent {
    let method = rng.pick(&["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "HEAD"]);
    let path = rng.pick(&["/", "/small", "/p/a%20b", "/no/such/page"]);
    let n = rng.below(24);
    let query = String::from_utf8(rng.bytes(n, b"ab=&%2F+.")).unwrap();
    let mut wire = format!("{method} {path}");
    if !query.is_empty() || rng.one_in(4) {
        wire += "?";
        wire += &query;
    }
    wire += " HTTP/1.1\r\n";
    let mut headers = rng.below(12);
    for _ in 0..headers {
        let n = 1 + rng.below(10);
        let name = String::from_utf8(rng.bytes(n, b"abcxyz-_")).unwrap();
        let n = rng.below(40);
        let value = String::from_utf8(rng.bytes(n, b"abc 012=;,/\t")).unwrap();
        wire += &format!(
            "x-{name}:{}{}\r\n",
            rng.pick(&["", " ", "  "]),
            value.trim()
        );
    }
    let n = rng.pick(&[0, 1, 10, SMALL, SMALL + 1, 500]);
    let body = rng.bytes(n, b"");
    if rng.one_in(2) {
        headers += 1;
        wire += "transfer-encoding: chunked\r\n\r\n";
        let mut wire = wire.into_bytes();
        let mut left = &body[..];
        while !left.is_empty() {
            let size = (1 + rng.below(80)).min(left.len());
            let size_text = if rng.one_in(2) {
                format!("{size:x}")
            } else {
                format!("{size:X}")
            };
            wire.extend_from_slice(size_text.as_bytes());
            if rng.one_in(4) {
                wire.extend_from_slice(b";ext=\"v\"");
            }
            wire.extend_from_slice(b"\r\n");
            wire.extend_from_slice(&left[..size]);
            wire.extend_from_slice(b"\r\n");
            left = &left[size..];
        }
        wire.extend_from_slice(b"0\r\n");
        if rng.one_in(3) {
            wire.extend_from_slice(b"x-trailer: 1\r\n");
        }
        wire.extend_from_slice(b"\r\n");
        return Sent {
            wire,
            path,
            query,
            body,
            headers,
        };
    }
    if !body.is_empty() || rng.one_in(3) {
        headers += 1;
        wire += &format!("Content-Length: {}\r\n", body.len());
    }
    wire += "\r\n";
    let mut wire = wire.into_bytes();
    wire.extend_from_slice(&body);
    Sent {
        wire,
        path,
        query,
        body,
        headers,
    }
}

fn limit(path: &str) -> usize {
    if path == "/small" {
        SMALL
    } else {
        crate::settings().body_limit
    }
}

fn cx_with(bytes: &[u8]) -> Cx {
    let mut cx = Cx::new(SocketAddr::from(([127, 0, 0, 1], 1)));
    cx.wire.buf.extend_from_slice(bytes);
    cx
}

/// What a parsed request says must lie inside the buffer, within limits.
fn check_parsed(cx: &Cx, at: usize, len: usize) {
    assert!(at + len <= cx.wire.buf.len());
    let body = cx.wire.body.range();
    assert!(body.start >= at && body.end <= at + len);
    assert!(body.len() <= limit(cx.path()));
    for (n, v) in &cx.wire.headers {
        assert!(n.range().end <= at + len && v.range().end <= at + len);
    }
    let _ = (cx.query_string(), cx.headers().count(), cx.cookie("a"));
    let _ = (
        cx.form().iter().count(),
        cx.host(),
        cx.bearer(),
        cx.basic_auth(),
    );
}

#[test]
fn requests_parse_whole_or_wait_for_more() {
    let mut rng = Rng::new(1);
    for _ in 0..3000 {
        let s = sent(&mut rng);
        let too_large = s.body.len() > limit(s.path);
        let mut cx = cx_with(&s.wire);
        match parse::<Fuzz>(&mut cx, 0, false) {
            Parsed::Request(Req { len, .. }) if !too_large => {
                assert_eq!(len, s.wire.len());
                assert_eq!(
                    (cx.path(), cx.query_string(), cx.body()),
                    (s.path, &*s.query, &s.body[..])
                );
                assert_eq!(cx.wire.headers.len(), s.headers);
                check_parsed(&cx, 0, len);
            }
            Parsed::Invalid(413) if too_large => {}
            _ => panic!("{:?}", String::from_utf8_lossy(&s.wire)),
        }
        // Any prefix is a request still arriving (or already too large).
        let cuts: Vec<usize> = if s.wire.len() < 200 {
            (0..s.wire.len()).collect()
        } else {
            (0..40).map(|_| rng.below(s.wire.len())).collect()
        };
        for cut in cuts {
            let mut cx = cx_with(&s.wire[..cut]);
            match parse::<Fuzz>(&mut cx, 0, false) {
                Parsed::Partial { .. } => {}
                Parsed::Invalid(413) if too_large => {}
                _ => panic!("{cut} of {:?}", String::from_utf8_lossy(&s.wire)),
            }
        }
    }
}

/// `header_name` says what comparing the names byte by byte, in any
/// case, says: of each name in mixed case, with each byte any other
/// `tchar`, one byte shorter or longer, and at the end of the buffer.
#[test]
fn header_names_read_as_compared() {
    let names = [
        ("host", Name::Host),
        ("content-length", Name::ContentLength),
        ("transfer-encoding", Name::TransferEncoding),
        ("connection", Name::Connection),
        ("expect", Name::Expect),
        ("if-none-match", Name::Known(Known::IfNoneMatch)),
        ("content-type", Name::Known(Known::ContentType)),
        ("accept", Name::Known(Known::Accept)),
        (crate::protocol::HEADER_ERROR, Name::Known(Known::WispError)),
        ("origin", Name::Known(Known::Origin)),
        ("sec-fetch-site", Name::Known(Known::SecFetchSite)),
    ];
    let want = |n: &[u8]| {
        names
            .iter()
            .find(|(s, _)| s.as_bytes().eq_ignore_ascii_case(n))
            .map_or(Name::Other, |&(_, k)| k)
    };
    let check = |n: &[u8], after: &[u8]| {
        let b = [b"x", n, after].concat();
        assert_eq!(
            header_name(&b, 1, n.len()),
            want(n),
            "{:?}",
            n.escape_ascii()
        );
    };
    let tchars: Vec<u8> = (0..=255u8).filter(|&c| TOKEN[usize::from(c)]).collect();
    let mut rng = Rng::new(11);
    for (s, _) in names {
        for _ in 0..50 {
            let n: Vec<u8> = s
                .bytes()
                .map(|c| {
                    if rng.one_in(2) {
                        c.to_ascii_uppercase()
                    } else {
                        c
                    }
                })
                .collect();
            check(&n, b": 1\r\n\r\n");
            check(&n, b"");
            check(&n[1..], b":");
            check(&n[..n.len() - 1], b":");
            check(&[&n[..], b"a"].concat(), b":");
        }
        for p in 0..s.len() {
            for &v in &tchars {
                let mut n = s.as_bytes().to_vec();
                n[p] = v;
                check(&n, b": 1\r\n\r\n");
                check(&n, b"");
            }
        }
    }
}

/// What `fast_head` reads, httparse reads the same, byte for byte; and
/// it reads every well-formed request `sent` makes, so it is the path
/// taken.
#[test]
fn the_fast_head_reads_as_httparse_does() {
    let same = |wire: &[u8], must: bool| {
        let (mut fast, mut slow, mut seen) = (Vec::new(), Vec::new(), Seen::default());
        let Some(f) = fast_head(wire, 0, &mut fast, &mut seen) else {
            assert!(!must, "{:?}", String::from_utf8_lossy(wire));
            return;
        };
        let Ok(s) = slow_head(wire, 0, &mut slow) else {
            panic!("httparse refused {:?}", String::from_utf8_lossy(wire));
        };
        let text = |s: Span| &wire[s.range()];
        // An empty value's span is anywhere.
        let empty = |mut s: Seen| {
            s.known = s
                .known
                .map(|k| if k.len == 0 { Span::default() } else { k });
            s
        };
        let pairs = |h: &[(Span, Span)]| {
            h.iter()
                .map(|&(n, v)| (text(n), text(v)))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            (
                f.len,
                f.method,
                f.http11,
                text(f.path),
                text(f.query),
                pairs(&fast),
                empty(seen)
            ),
            (
                s.len,
                s.method,
                s.http11,
                text(s.path),
                text(s.query),
                pairs(&slow),
                empty(Seen::of(wire, &slow))
            ),
            "{:?}",
            String::from_utf8_lossy(wire)
        );
    };
    let mut rng = Rng::new(6);
    for k in 0..30_000 {
        let mut wire = sent(&mut rng).wire;
        let whole = k % 3 == 0;
        if !whole {
            mutate(&mut rng, &mut wire);
        }
        same(&wire, whole);
    }
    // The headers read by name, in any case and order, and broken.
    let names = [
        "Host",
        "Content-Length",
        "Connection",
        "Content-Type",
        "Hosts",
        "Content-Lengths",
        "Connections",
        "Hos",
        "User-Agent",
        "User-Agents",
    ];
    for k in 0..30_000 {
        let mut wire = b"POST /x HTTP/1.1\r\n".to_vec();
        for _ in 0..rng.below(5) {
            wire.extend(rng.pick(&names).bytes().map(|c| {
                if rng.one_in(2) {
                    c.to_ascii_uppercase()
                } else {
                    c.to_ascii_lowercase()
                }
            }));
            wire.extend_from_slice(rng.pick(&[&b": 1"[..], b":", b":\tclose"]));
            wire.extend_from_slice(b"\r\n");
        }
        wire.extend_from_slice(b"\r\n");
        let whole = k % 3 == 0;
        if !whole {
            mutate(&mut rng, &mut wire);
        }
        same(&wire, whole);
    }
    // Every byte in a method, target, header name and value, and around
    // the lines' ends.
    for v in 0..=255u8 {
        let c = char::from(v);
        for wire in [
            format!("G{c}T / HTTP/1.1\r\n\r\n").into_bytes(),
            format!("GET /a{c}b HTTP/1.1\r\n\r\n").into_bytes(),
            format!("GET / HTTP/1.1\r\nx{c}y: 1\r\n\r\n").into_bytes(),
            format!("GET / HTTP/1.1\r\nx:{c}a\r\n\r\n").into_bytes(),
            format!("GET / HTTP/1.1\r\nx: 0123456789{c}abcdef{c} \r\n\r\n").into_bytes(),
            format!("GET / HTTP/1.1{c}\nx: 1\r{c}\r\n").into_bytes(),
            [
                &b"GET / HTTP/1.1\r\nx: "[..],
                &[v, b'\r', b'\n', b'\r', b'\n'],
            ]
            .concat(),
            [
                &b"GET /"[..],
                &[v; 9],
                b" HTTP/1.0\r\nx: ",
                &[v; 17],
                b"\r\n\r\n",
            ]
            .concat(),
        ] {
            same(&wire, false);
        }
    }
    same(
        b"GET /x?a=1&b HTTP/1.1\r\nHost: a\r\nX-Empty:\r\nX-Tabs:\t a\tb \t\r\n\r\n",
        true,
    );
    same(b"DELETE /x HTTP/1.0\r\n\r\n", true);
}

#[test]
fn pipelined_requests_parse_in_turn() {
    let mut rng = Rng::new(2);
    for _ in 0..1000 {
        let (a, b) = (sent(&mut rng), sent(&mut rng));
        if a.body.len() > limit(a.path) || b.body.len() > limit(b.path) {
            continue;
        }
        let mut cx = cx_with(&[&a.wire[..], &b.wire].concat());
        let Parsed::Request(Req { len, .. }) = parse::<Fuzz>(&mut cx, 0, false) else {
            panic!()
        };
        assert_eq!((len, cx.body()), (a.wire.len(), &a.body[..]));
        let Parsed::Request(Req { len, .. }) = parse::<Fuzz>(&mut cx, len, false) else {
            panic!()
        };
        assert_eq!((len, cx.body()), (b.wire.len(), &b.body[..]));
    }
}

#[test]
fn broken_requests_never_panic() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let mut rng = Rng::new(3);
    let (mut out, mut reply, mut w) = (Out::default(), Reply::default(), Vec::new());
    for _ in 0..20_000 {
        let mut wire = sent(&mut rng).wire;
        mutate(&mut rng, &mut wire);
        let mut cx = cx_with(&wire);
        match parse::<Fuzz>(&mut cx, 0, false) {
            Parsed::Request(Req { len, .. }) => {
                check_parsed(&cx, 0, len);
                rt.block_on(decide::<Fuzz>(&mut cx, &mut out, &mut reply, None));
                w.clear();
                serialize::<Fuzz, true>(&mut w, &mut reply, &mut out, cx.wire.http11, true, false);
                assert!(w.starts_with(b"HTTP/1.1 "));
            }
            Parsed::Partial { need, .. } => assert!(need <= MAX_HEAD + MAX_BODY),
            Parsed::Invalid(status) => assert!(matches!(status, 400 | 413 | 431 | 501)),
        }
    }
}

#[test]
fn heads_and_chunk_framing_are_bounded() {
    // A head that never ends is refused once it passes MAX_HEAD.
    let mut long = b"GET / HTTP/1.1\r\n".to_vec();
    while long.len() <= MAX_HEAD {
        long.extend_from_slice(b"x-a: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\r\n");
    }
    assert!(matches!(
        parse::<Fuzz>(&mut cx_with(&long), 0, false),
        Parsed::Invalid(431)
    ));
    let many: String = (0..MAX_HEADERS + 1)
        .map(|i| format!("x-{i}: 1\r\n"))
        .collect();
    let many = format!("GET / HTTP/1.1\r\n{many}\r\n");
    assert!(matches!(
        parse::<Fuzz>(&mut cx_with(many.as_bytes()), 0, false),
        Parsed::Invalid(431)
    ));

    let head = "POST /small HTTP/1.1\r\ntransfer-encoding: chunked\r\n\r\n";
    let body_of = |chunked: &str| {
        let mut cx = cx_with(format!("{head}{chunked}").as_bytes());
        match parse::<Fuzz>(&mut cx, 0, false) {
            Parsed::Request(_) => Ok(cx.body().to_vec()),
            Parsed::Invalid(status) => Err(status),
            Parsed::Partial { .. } => Err(0),
        }
    };
    assert_eq!(
        body_of("3\r\nabc\r\n2;x=y\r\nde\r\n0\r\nt: 1\r\n\r\n"),
        Ok(b"abcde".to_vec())
    );
    assert_eq!(body_of("41\r\n"), Err(413), "over the route's limit");
    assert_eq!(body_of("ffffffffffffffff\r\n"), Err(413));
    assert_eq!(body_of("10000000000000000\r\n"), Err(400), "17 digits");
    assert_eq!(body_of("3\r\nabcX\r\n0\r\n\r\n"), Err(400));
    assert_eq!(body_of("3\nabc\r\n0\r\n\r\n"), Err(400), "bare LF");
    assert_eq!(body_of("-3\r\nabc\r\n0\r\n\r\n"), Err(400));
    assert_eq!(body_of("3\r\nabc\r\n0\r\n"), Err(0), "no end yet");
    // Framing that is most of the wire, with empty chunks, is refused.
    let padded = format!("1;{}\r\na\r\n", "e".repeat(MAX_HEAD - 8)).repeat(3);
    assert_eq!(body_of(&padded), Err(413));

    let mut rng = Rng::new(4);
    for _ in 0..20_000 {
        let n = rng.below(64);
        let b = rng.bytes(n, b"0123456789abcdefgxX;= \r\n");
        if let Chunks::Complete { wire, body } = chunks(&b, 1 << 20) {
            assert!(wire <= b.len() && body <= wire);
            let mut copy = b.clone();
            unchunk(&mut copy[..wire]);
        }
    }
}

/// Every framing a proxy in front could read otherwise is refused, on
/// the fast head and httparse's alike; the usual ones are taken.
#[test]
fn smuggling_shapes_are_refused() {
    let outcome = |wire: &str| match parse::<Fuzz>(&mut cx_with(wire.as_bytes()), 0, true) {
        Parsed::Request(r) => Ok(r.len),
        Parsed::Invalid(status) => Err(status),
        Parsed::Partial { .. } => Err(0),
    };
    let post = |headers: &str, body: &str| {
        outcome(&format!(
            "POST /x HTTP/1.1\r\nhost: a\r\n{headers}\r\n{body}"
        ))
    };
    let refused = |r: Result<usize, u16>| matches!(r, Err(400 | 501));
    for (headers, body) in [
        // CL.TE and TE.CL, either order, any case.
        (
            "content-length: 3\r\ntransfer-encoding: chunked\r\n",
            "0\r\n\r\n",
        ),
        (
            "Transfer-Encoding: chunked\r\nContent-Length: 3\r\n",
            "0\r\n\r\n",
        ),
        // Two lengths that differ, or a list, or not digits.
        ("content-length: 3\r\ncontent-length: 4\r\n", "abcd"),
        ("content-length: 3, 3\r\n", "abc"),
        ("content-length: +3\r\n", "abc"),
        ("content-length: 0x3\r\n", "abc"),
        ("content-length: 3 3\r\n", "abc"),
        ("content-length: -1\r\n", ""),
        ("content-length: 99999999999999999999\r\n", ""),
        // Codings other than chunked alone, and chunked twice.
        ("transfer-encoding: chunked, chunked\r\n", "0\r\n\r\n"),
        (
            "transfer-encoding: chunked\r\ntransfer-encoding: chunked\r\n",
            "0\r\n\r\n",
        ),
        ("transfer-encoding: xchunked\r\n", "0\r\n\r\n"),
        ("transfer-encoding: chunked;q=1\r\n", "0\r\n\r\n"),
        ("transfer-encoding: identity\r\n", ""),
        ("transfer-encoding:\r\n", ""),
        // A name with space before its colon, a folded line, bare CRs,
        // NUL and DEL in a value, a line with no name.
        ("transfer-encoding : chunked\r\n", "0\r\n\r\n"),
        ("content-length : 3\r\n", "abc"),
        ("x: a\r\n chunked\r\n", ""),
        ("x: a\r\n\ttransfer-encoding: chunked\r\n", ""),
        (
            "content-length: 3\r\n transfer-encoding: chunked\r\n",
            "abc",
        ),
        ("x: a\rtransfer-encoding: chunked\r\n", "0\r\n\r\n"),
        ("x: a\0b\r\n", ""),
        ("x: a\x7fb\r\n", ""),
        ("\rx: a\r\n", ""),
        (": a\r\n", ""),
    ] {
        let got = post(headers, body);
        assert!(refused(got), "{headers:?} {body:?}: {got:?}");
    }
    // Chunk sizes a proxy reads otherwise: signs, spaces, prefixes, and
    // lines with bare LF or CR.
    let chunked = |body: &str| post("transfer-encoding: chunked\r\n", body);
    for body in [
        "+3\r\nabc\r\n0\r\n\r\n",
        "0x3\r\nabc\r\n0\r\n\r\n",
        " 3\r\nabc\r\n0\r\n\r\n",
        "3 \r\nabc\r\n0\r\n\r\n",
        "3\rabc\r\n0\r\n\r\n",
        "3\r\nabc\n0\r\n\r\n",
        "3\r\nabc\r\n0\r\nx: 1\n\r\n",
        "3;a\rb\r\nabc\r\n0\r\n\r\n",
        "3\r\nabcd\r\n0\r\n\r\n",
        "g\r\n",
    ] {
        let got = chunked(body);
        assert!(refused(got), "{body:?}: {got:?}");
    }
    // HTTP/1.0 has no chunks; two hosts are two answers to "which site".
    assert!(refused(outcome(
        "POST /x HTTP/1.0\r\ntransfer-encoding: chunked\r\n\r\n0\r\n\r\n"
    )));
    assert!(refused(outcome(
        "GET / HTTP/1.1\r\nhost: a\r\nhost: b\r\n\r\n"
    )));
    // What is fine reads to its end, and no further.
    for (headers, body) in [
        ("content-length: 3\r\ncontent-length: 3\r\n", "abc"),
        ("Content-Length:3  \r\n", "abc"),
        (
            "transfer-encoding:  Chunked \r\n",
            "3;a=b\r\nabc\r\n0\r\nt: 1\r\n\r\n",
        ),
    ] {
        let wire = format!("POST /x HTTP/1.1\r\nhost: a\r\n{headers}\r\n{body}");
        assert_eq!(post(headers, body), Ok(wire.len()), "{headers:?}");
        let next = format!("{wire}GET / HTTP/1.1\r\n");
        assert_eq!(outcome(&next), Ok(wire.len()), "{headers:?}");
    }
}

#[test]
fn http_1_1_names_one_host() {
    // On the wire; from another host (`Cx::from_request`) it may not.
    let hosted = |wire: &[u8], on_wire| match parse::<Fuzz>(&mut cx_with(wire), 0, on_wire) {
        Parsed::Request(_) => Ok(()),
        Parsed::Invalid(status) => Err(status),
        Parsed::Partial { .. } => Err(0),
    };
    assert_eq!(hosted(b"GET / HTTP/1.1\r\nHost: a\r\n\r\n", true), Ok(()));
    assert_eq!(hosted(b"GET / HTTP/1.1\r\nx: 1\r\n\r\n", true), Err(400));
    assert_eq!(hosted(b"GET / HTTP/1.1\r\nx: 1\r\n\r\n", false), Ok(()));
    assert_eq!(hosted(b"GET / HTTP/1.0\r\n\r\n", true), Ok(()));
    assert_eq!(
        hosted(b"GET / HTTP/1.1\r\nhost: a\r\nHOST: a\r\n\r\n", false),
        Err(400)
    );
    assert_eq!(
        hosted(b"GET / HTTP/1.0\r\nhost: a\r\nhost: b\r\n\r\n", true),
        Err(400)
    );
}

#[test]
fn an_absolute_target_is_its_path_on_its_host() {
    let read = |wire: &[u8]| {
        let mut cx = cx_with(wire);
        match parse::<Fuzz>(&mut cx, 0, true) {
            Parsed::Request(_) => Some(format!(
                "{} {} {}",
                cx.header("host").unwrap_or("-"),
                cx.path(),
                cx.query_string()
            )),
            _ => None,
        }
    };
    let host =
        |target: &str| read(format!("GET {target} HTTP/1.1\r\nHost: proxy\r\n\r\n").as_bytes());
    assert_eq!(
        host("http://a.test/x/y?q=1").as_deref(),
        Some("a.test /x/y q=1")
    );
    assert_eq!(
        host("HTTPS://a.test:8443").as_deref(),
        Some("a.test:8443 / ")
    );
    assert_eq!(host("http://a.test?q").as_deref(), Some("a.test / q"));
    // Anything else is left as it came, and refused later.
    for odd in [
        "*",
        "ftp://a/",
        "http://",
        "http://u@a/",
        "http:/a/",
        "a.test/x",
    ] {
        assert_eq!(host(odd), Some(format!("proxy {odd} ")), "{odd}");
    }
    // HTTP/1.1 still names a host, as it must.
    assert_eq!(read(b"GET http://a/ HTTP/1.1\r\n\r\n"), None);
    assert_eq!(
        read(b"GET http://a/ HTTP/1.0\r\n\r\n").as_deref(),
        Some("- / ")
    );
}

#[test]
fn slash_redirects_stay_on_the_site_and_framing_is_ours() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let (mut out, mut reply) = (Out::default(), Reply::default());
    for (path, to) in [
        ("//evil.example/", "/evil.example"),
        ("/\\evil.example/", "/evil.example"),
        ("/a/b//?x=1", "/a/b?x=1"),
        ("///", "/"),
    ] {
        let mut cx = cx_with(format!("GET {path} HTTP/1.1\r\n\r\n").as_bytes());
        assert!(matches!(
            parse::<Fuzz>(&mut cx, 0, false),
            Parsed::Request(_)
        ));
        rt.block_on(decide::<Fuzz>(&mut cx, &mut out, &mut reply, None));
        assert_eq!((reply.status, reply.header("location")), (308, Some(to)));
    }

    let mut w = Vec::new();
    let mut reply = Reply::plain(200);
    for (n, v) in [("content-length", "99"), ("Transfer-Encoding", "chunked")] {
        reply.headers.push((Cow::Borrowed(n), Cow::Borrowed(v)));
    }
    serialize::<Fuzz, true>(&mut w, &mut reply, &mut out, false, true, false);
    let text = String::from_utf8(w).unwrap().to_ascii_lowercase();
    assert!(text.contains("content-length: 2\r\n") && text.contains("connection: keep-alive"));
    assert!(!text.contains("99") && !text.contains("chunked"), "{text}");
}

/// RFC 9110 §15.3.6: a 205 has no content, and says so with
/// `content-length: 0`, else a client may wait for one; 204 and 304
/// carry no length at all.
#[test]
fn reset_content_is_bodiless_with_a_zero_length() {
    let mut out = Out::default();
    for (status, length) in [(205, Some("0")), (204, None), (304, None)] {
        let mut w = Vec::new();
        let mut reply = Reply::plain(status);
        reply.body = Body::Bytes(b"dropped".to_vec());
        serialize::<Fuzz, true>(&mut w, &mut reply, &mut out, true, true, false);
        let text = String::from_utf8(w).unwrap().to_ascii_lowercase();
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        assert_eq!(body, "", "{status}");
        let len = head
            .lines()
            .find_map(|l| l.strip_prefix("content-length: "));
        assert_eq!(len, length, "{status}: {head}");
    }
}

#[test]
fn a_request_id_is_echoed_when_asked_for() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let sent = |path: &str, id: &str| {
        let mut req = Request::new("GET", path);
        if !id.is_empty() {
            req.header("x-request-id", id);
        }
        rt.block_on(handle::<Fuzz>(req))
    };
    assert_eq!(sent("/p/id", "abc-1").header("x-request-id"), Some("abc-1"));
    assert_eq!(
        sent("/p/id", "").header("x-request-id").map(str::len),
        Some(16)
    );
    assert_eq!(sent("/p/other", "abc-1").header("x-request-id"), None);
}

/// The routes of `bench/app`, as `wisp-build` writes them: `/plaintext`
/// (0), `/json` (1) and `/fortunes` (2), a page of escaped rows.
struct Bench;

struct Message {
    message: &'static str,
}

impl crate::Json for Message {
    fn json(&self, out: &mut String) {
        out.push_str("{\"message\":");
        crate::Json::json(&self.message, out);
        out.push('}');
    }
}

const ROWS: [(u32, &str); 13] = [
    (0, "Additional fortune added at request time."),
    (1, "fortune: No such file or directory"),
    (
        2,
        "A computer scientist is someone who fixes things that aren't broken.",
    ),
    (3, "After enough decimal places, nobody gives a damn."),
    (
        4,
        "A bad random number generator: 1, 1, 1, 1, 1, 4.33e+67, 1, 1, 1",
    ),
    (
        5,
        "A computer program does what you tell it to do, not what you want it to do.",
    ),
    (
        6,
        "Emacs is a nice operating system, but I prefer UNIX. — Tom Christaensen",
    ),
    (7, "Any program that runs right is obsolete."),
    (
        8,
        "A list is only as strong as its weakest link. — Donald Knuth",
    ),
    (9, "Feature: A bug with seniority."),
    (10, "Computers make very fast, very accurate mistakes."),
    (
        11,
        "<script>alert(\"This should not be displayed in a browser alert box.\");</script>",
    ),
    (12, "フレームワークのベンチマーク"),
];

impl App for Bench {
    const ROOT: &'static str = ".";
    const CSS: Option<&'static str> = None;
    const ROUTES: &'static [crate::rt::RouteFacts] =
        &[const { crate::rt::RouteFacts::new(&[]) }; 3];
    const TEMPLATES: &'static [(&'static str, u64)] = &[];

    fn route(path: &str) -> Option<(usize, [&str; 8])> {
        let mut segs = [""; crate::rt::MAX_SEGS];
        let id = match crate::rt::split(path, &mut segs)? {
            ["plaintext"] => 0,
            ["json"] => 1,
            ["fortunes"] => 2,
            _ => return None,
        };
        Some((id, [""; 8]))
    }

    fn shell() -> [&'static str; 3] {
        [
            "<!DOCTYPE html>\n<html>\n<head>",
            "</head>\n<body>",
            "</body>\n</html>\n",
        ]
    }

    fn asset(_: &str) -> Option<&'static crate::Asset> {
        None
    }

    async fn init() -> crate::Result<()> {
        Ok(())
    }

    #[allow(clippy::needless_borrow)] // the form the generated code has
    async fn handle(route: Option<usize>, cx: &mut Cx, out: &mut Out) -> crate::Result<()> {
        use crate::html::{Direct as _, Text};
        use crate::rt_traits::ret::{Ret, Shape as _, Value as _};
        crate::rt::hooked(cx);
        let Some(route) = route else {
            return Err(Error::new(404, "Not Found"));
        };
        if cx.method == Method::Get && cx.header(crate::protocol::HEADER_ERROR).is_some() {
            return Err(Error::new(500, "Something went wrong in the browser"));
        }
        match route {
            0 => {
                crate::rt::endpoint(cx);
                let r = (&&&Ret::new(crate::Response::text("Hello, World!"))).respond()?;
                crate::rt::respond(out, r);
            }
            1 => {
                crate::rt::endpoint(cx);
                let r = (&&&Ret::new(Message {
                    message: "Hello, World!",
                }))
                    .respond()?;
                crate::rt::respond(out, r);
            }
            _ => {
                out.head.push_str("<title>Fortunes</title>");
                out.body
                    .push_str("\n<table>\n<tr><th>id</th><th>message</th></tr>\n");
                for (id, message) in &ROWS {
                    out.body.push_str("<tr><td>");
                    (&Text(id)).put(&mut out.body);
                    out.body.push_str("</td><td>");
                    (&Text(message)).put(&mut out.body);
                    out.body.push_str("</td></tr>\n");
                }
                out.body.push_str("</table>");
            }
        }
        Ok(())
    }

    async fn error(
        _: Option<usize>,
        cx: &mut Cx,
        out: &mut Out,
        status: u16,
        message: &str,
    ) -> crate::Result<()> {
        crate::rt::default_error(cx, out, status, message);
        Ok(())
    }
}

/// Polls `f` once: the routes of `Bench` never wait.
fn ready<F: Future>(f: F) -> F::Output {
    let mut f = std::pin::pin!(f);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match f.as_mut().poll(&mut cx) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!("a Bench route waited"),
    }
}

/// One request on a kept-alive connection, as `requests` answers it:
/// parsed from the read buffer, decided, written to the write buffer.
fn answer(b: &mut Buffers, request: &[u8]) {
    b.cx.wire.buf.clear();
    b.cx.wire.buf.extend_from_slice(request);
    let Parsed::Request(Req { keep_alive, .. }) = parse::<Bench>(&mut b.cx, 0, false) else {
        panic!("{:?}", String::from_utf8_lossy(request));
    };
    ready(decide::<Bench>(&mut b.cx, &mut b.out, &mut b.reply, None));
    b.wbuf.clear();
    let head = b.cx.method == Method::Head;
    serialize::<Bench, true>(
        &mut b.wbuf,
        &mut b.reply,
        &mut b.out,
        b.cx.wire.http11,
        keep_alive,
        head,
    );
    b.cx.reset();
}

fn buffers() -> Buffers {
    Buffers {
        cx: Cx::new(SocketAddr::from(([127, 0, 0, 1], 1))),
        wbuf: Vec::with_capacity(16 * 1024),
        out: Out::default(),
        reply: Reply::default(),
    }
}

/// Once warm, a kept-alive connection answers `/plaintext`, `/json` and
/// a page of escaped rows, HEAD or GET, with no allocation: its
/// buffers, `Cx`, headers and response bodies are all reused. Measured
/// as a release server runs, with dev mode off: in a child process, when
/// this one has it on, and in another with `METRICS_KEY` set, whose
/// counting allocates nothing either.

#[test]
fn warm_requests_allocate_nothing() {
    if crate::settings().dev {
        let name = "http::tests::warm_requests_allocate_nothing";
        for metrics in ["", "key"] {
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([name, "--exact", "--nocapture"])
                .env("WISP_DEV", "off")
                .env("METRICS_KEY", metrics)
                .output()
                .unwrap();
            let said = String::from_utf8_lossy(&child.stdout);
            assert!(
                child.status.success() && said.contains("1 passed"),
                "{said}"
            );
        }
        return;
    }
    crate::obs::init(Bench::ROUTES);
    let mut b = buffers();
    for (method, path, answered) in [
        ("GET", "/plaintext", &b"Hello, World!"[..]),
        ("GET", "/json", br#"{"message":"Hello, World!"}"#),
        ("GET", "/fortunes", b"&lt;script&gt;alert(&quot;This"),
        ("HEAD", "/json", b"content-length: 27\r\n"),
    ] {
        let request = format!("{method} {path} HTTP/1.1\r\nhost: localhost\r\naccept: */*\r\n\r\n");
        for _ in 0..3 {
            answer(&mut b, request.as_bytes());
        }
        let made = allocation_counter::measure(|| {
            for _ in 0..100 {
                answer(&mut b, request.as_bytes());
            }
        });
        let text = String::from_utf8_lossy(&b.wbuf);
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "{text}");
        assert!(
            b.wbuf.windows(answered.len()).any(|w| w == answered),
            "{text}"
        );
        assert_eq!(made.count_total, 0, "{method} {path}");
    }
}

/// What a request costs Wisp itself, sockets aside: parse, decide and
/// serialize on a warm connection, as zrk sends it.
/// `cargo test -p wisp --release --lib http::tests::cost -- --ignored --nocapture`
#[test]
#[ignore]
fn cost() {
    let mut b = buffers();
    for path in ["/plaintext", "/json", "/fortunes"] {
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:3000\r\nUser-Agent: zrk\r\nConnection: keep-alive\r\n\r\n"
        );
        for _ in 0..10_000 {
            answer(&mut b, request.as_bytes());
        }
        let n = 1_000_000;
        let started = std::time::Instant::now();
        for _ in 0..n {
            answer(&mut b, std::hint::black_box(request.as_bytes()));
        }
        let ns = started.elapsed().as_nanos() as f64 / n as f64;
        println!("{path}: {ns:.0} ns a request");
    }
}

/// Routes the build said never wait: `/now` (0), and `/wait` (1),
/// which waits after all, as a wrong guess would.
#[cfg(target_os = "linux")]
struct Guessed;

#[cfg(target_os = "linux")]
static WAITED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(target_os = "linux")]
impl App for Guessed {
    const ROOT: &'static str = ".";
    const CSS: Option<&'static str> = None;
    const ROUTES: &'static [crate::rt::RouteFacts] = &[const {
        crate::rt::RouteFacts {
            now: true,
            ..crate::rt::RouteFacts::new(&[])
        }
    }; 2];
    const TEMPLATES: &'static [(&'static str, u64)] = &[];

    fn route(path: &str) -> Option<(usize, [&str; 8])> {
        match path {
            "/now" => Some((0, [""; 8])),
            "/wait" => Some((1, [""; 8])),
            _ => None,
        }
    }

    fn shell() -> [&'static str; 3] {
        ["", "", ""]
    }

    fn asset(_: &str) -> Option<&'static crate::Asset> {
        None
    }

    async fn init() -> crate::Result<()> {
        Ok(())
    }

    async fn handle(route: Option<usize>, cx: &mut Cx, out: &mut Out) -> crate::Result<()> {
        crate::rt::hooked(cx);
        crate::rt::endpoint(cx);
        let text = match route {
            Some(0) => "now",
            Some(_) => {
                tokio::time::sleep(Duration::from_millis(20)).await;
                WAITED.fetch_add(1, Ordering::Relaxed);
                "waited"
            }
            None => return Err(Error::new(404, "Not Found")),
        };
        crate::rt::respond(out, crate::Response::text(text));
        Ok(())
    }

    async fn error(
        _: Option<usize>,
        cx: &mut Cx,
        out: &mut Out,
        status: u16,
        message: &str,
    ) -> crate::Result<()> {
        crate::rt::default_error(cx, out, status, message);
        Ok(())
    }
}

/// A request to a route the build wrongly said never waits is no
/// failure: the driver hands it, still deciding, to the connection's
/// future, which answers it once, then the requests after it, in order;
/// and the driver answers the next ones itself again.
#[cfg(target_os = "linux")]
#[test]
fn a_route_that_waits_after_all_is_answered_in_order() {
    use std::io::{Read, Write};
    let listener = crate::uring::listen("127.0.0.1:0".parse().unwrap()).unwrap();
    let addr = listener.local_addr().unwrap();
    start_clock();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let (ready, _) = std::sync::mpsc::channel();
            let now = Some(on_driver::<Guessed> as fn(u64) -> bool);
            tokio::spawn(crate::epoll::serve(listener, polled::<Guessed>, now, ready));
            std::future::pending::<()>().await
        });
    });
    let mut c = std::net::TcpStream::connect(addr).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let get = |path: &str| format!("GET {path} HTTP/1.1\r\nhost: a\r\n\r\n");
    // The bodies of the answers, in order, each by its content-length.
    let mut ask = |paths: &[&str]| {
        let wire: String = paths.iter().map(|p| get(p)).collect();
        c.write_all(wire.as_bytes()).unwrap();
        let (mut got, mut bodies, mut buf) = (String::new(), Vec::new(), [0; 4096]);
        while bodies.len() < paths.len() {
            if let Some(end) = got.find("\r\n\r\n") {
                let len: usize = got[..end]
                    .split("content-length: ")
                    .nth(1)
                    .and_then(|l| l.split("\r\n").next()?.parse().ok())
                    .unwrap();
                if got.len() >= end + 4 + len {
                    assert!(got.starts_with("HTTP/1.1 200 OK"), "{got}");
                    bodies.push(got[end + 4..end + 4 + len].to_string());
                    got.drain(..end + 4 + len);
                    continue;
                }
            }
            let n = c.read(&mut buf).unwrap();
            assert!(n > 0, "closed after {got:?}");
            got.push_str(std::str::from_utf8(&buf[..n]).unwrap());
        }
        bodies.join(" ")
    };
    let before = WAITED.load(Ordering::Relaxed);
    assert_eq!(ask(&["/now"]), "now");
    std::thread::sleep(Duration::from_millis(50)); // its future waits bare
    assert_eq!(ask(&["/now", "/wait", "/now"]), "now waited now");
    assert_eq!(WAITED.load(Ordering::Relaxed) - before, 1);
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(ask(&["/now", "/now"]), "now now");
    assert_eq!(ask(&["/wait"]), "waited");
}

/// Routes whose GET the build made sync: `/sync` (0), which
/// `handle_now` answers; `/later` (1), which it turns out not to, so
/// `handle` does; `/boom` (2), which panics; `/gone` (3), which fails
/// to its error page, rendered in the decider.
#[cfg(target_os = "linux")]
struct Plain;

#[cfg(target_os = "linux")]
impl App for Plain {
    const ROOT: &'static str = ".";
    const CSS: Option<&'static str> = None;
    const ROUTES: &'static [crate::rt::RouteFacts] = &[const {
        crate::rt::RouteFacts {
            now: true,
            sync: Method::Get.bit(),
            ..crate::rt::RouteFacts::new(&[])
        }
    }; 4];
    const TEMPLATES: &'static [(&'static str, u64)] = &[];

    fn route(path: &str) -> Option<(usize, [&str; 8])> {
        let id = ["/sync", "/later", "/boom", "/gone"]
            .iter()
            .position(|p| *p == path)?;
        Some((id, [""; 8]))
    }

    fn shell() -> [&'static str; 3] {
        ["", "", ""]
    }

    fn asset(_: &str) -> Option<&'static crate::Asset> {
        None
    }

    async fn init() -> crate::Result<()> {
        Ok(())
    }

    async fn handle(_: Option<usize>, _: &mut Cx, out: &mut Out) -> crate::Result<()> {
        crate::rt::respond(out, crate::Response::text("later"));
        Ok(())
    }

    async fn error(
        _: Option<usize>,
        _: &mut Cx,
        out: &mut Out,
        _: u16,
        _: &str,
    ) -> crate::Result<()> {
        out.body.push_str("page");
        Ok(())
    }

    fn handle_now(route: Option<usize>, _: &mut Cx, out: &mut Out) -> crate::Result<bool> {
        match route {
            Some(0) => crate::rt::respond(out, crate::Response::text("sync")),
            Some(2) => panic!("boom"),
            Some(3) => return Err(Error::new(410, "Gone")),
            _ => return Ok(false),
        }
        Ok(true)
    }
}

/// The driver answers sync arms with no future, falls back to `handle`
/// for one that has none, turns a panic into a 500, and has the decider
/// render an error page: all in order, none handed on.
#[cfg(target_os = "linux")]
#[test]
fn sync_arms_are_answered_without_a_future() {
    start_clock();
    let mut b = Box::new(buffers());
    let paths = ["/sync", "/later", "/boom", "/gone", "/sync"];
    let wire = paths.map(|p| format!("GET {p} HTTP/1.1\r\nhost: a\r\naccept: text/html\r\n\r\n"));
    b.cx.wire.buf.extend_from_slice(wire.concat().as_bytes());
    let Ok(b) = answer_whole::<Plain>(b) else {
        panic!("handed on");
    };
    let text = String::from_utf8_lossy(&b.wbuf);
    let statuses: Vec<&str> = text.split("HTTP/1.1 ").skip(1).map(|r| &r[..3]).collect();
    assert_eq!(statuses, ["200", "200", "500", "410", "200"], "{text}");
    let bodies: Vec<&str> = (text.split("\r\n\r\n").skip(1))
        .map(|r| r.split("HTTP/1.1").next().unwrap())
        .collect();
    // A page's head may have tags other tests set (`HEAD_TAGS`).
    let ends = ["sync", "later", "page", "page", "sync"];
    assert!(
        bodies.iter().zip(ends).all(|(b, e)| b.ends_with(e)),
        "{text}"
    );
}

/// A `String` input takes a spare body only of about its size: one it
/// keeps (in a table, say) never holds a large body's room.
#[test]
fn a_string_input_takes_a_spare_of_its_size_only() {
    recycle(Vec::with_capacity(4096));
    assert!(spare_for(3).capacity() < 64);
    assert_eq!(spare().capacity(), 4096);
    recycle(Vec::with_capacity(32));
    assert_eq!(spare_for(10).capacity(), 32);
    assert_eq!(spare().capacity(), 0);
}

#[test]
fn requests_from_other_hosts_never_panic() {
    let mut rng = Rng::new(5);
    for _ in 0..5000 {
        let method = String::from_utf8(rng.upto(8, b"GETPOSt \r")).unwrap();
        let n = rng.below(30);
        let target =
            String::from_utf8_lossy(&rng.bytes(n, b"/a?=%& \x7f\xc3\xa9\r\n")).into_owned();
        let name = rng.text(8);
        let value = rng.upto(20, b"");
        let headers = [(name.as_str(), &value[..])];
        let body = rng.upto(100, b"");
        if let Ok(cx) =
            Cx::from_request::<Fuzz>(&method, &target, headers, &body, cx_with(b"").peer())
        {
            assert_eq!(cx.body(), body);
            check_parsed(&cx, 0, cx.wire.buf.len());
        }
    }
}
