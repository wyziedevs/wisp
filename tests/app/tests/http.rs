//! The test app's binary, run as a server: hooks, state, signed cookies,
//! uploads, actions that answer with a file, chunked request bodies, body
//! limits, streamed responses and browser code (client scripts and
//! directives), each checked on the wire.

mod common;
#[path = "../../../tests/shared/ws.rs"]
mod ws;

use common::{MULTIPART, SECRET, Server, Temp, body, connect, header, multipart, status};
use std::io::{Read, Write};
use std::time::Duration;

fn start() -> Server {
    start_with(&[])
}

/// With these environment settings on top of the usual ones (and a WebSocket
/// that is idle for 2 s, not a minute).
fn start_with(env: &[(&str, &str)]) -> Server {
    let mut all = vec![("WISP_WS_IDLE", "2")];
    all.extend_from_slice(env);
    common::start(&all)
}

const FORM: &str = "content-type: application/x-www-form-urlencoded\r\n";

#[test]
fn saved_tables_survive_a_crash() {
    let dir = Temp::new("data");
    let data = dir.to_str().unwrap();
    let auth = "authorization: Bearer wisp-test-app\r\ncontent-type: application/json\r\n";
    {
        let s = start_with(&[("WISP_DATA", data), ("WISP_FSYNC", "always")]);
        let made = s.request("POST", "/notes", auth, br#"{"title":"Kept"}"#);
        assert_eq!(status(&made), 201, "{made}");
        s.request("POST", "/notes", auth, br#"{"title":"Gone"}"#);
        assert_eq!(status(&s.request("DELETE", "/notes/2", auth, b"")), 204);
    } // killed, as a crash would be
    let s = start_with(&[("WISP_DATA", data)]);
    let list = s.request("GET", "/notes", "", b"");
    assert_eq!(body(&list), r#"[{"id":1,"title":"Kept","done":false}]"#);
    let next = s.request("POST", "/notes", auth, br#"{"title":"Next"}"#);
    assert!(body(&next).starts_with(r#"{"id":3,"#), "ids go on: {next}");
    assert!(dir.join("note.log").exists());
}

/// A page's `<style>` is its own: its elements get its class, a
/// component's do not, and the CSS is in `/_app/app.css`.
#[test]
fn scoped_styles() {
    let s = start();
    let page = s.request("GET", "/styled", "", b"");
    let at = page.find("<h1 class=\"").expect("a class") + 11;
    let class = &page[at..at + 8];
    assert!(class.starts_with("w-"), "{page}");
    assert!(
        page.contains(&format!("<p class=\"lead {class}\">")),
        "{page}"
    );
    assert!(page.contains("<span class=\"badge\">kept</span>"), "{page}");
    assert!(
        page.contains("<link rel=\"stylesheet\" href=\"/_app/app.css?v="),
        "{page}"
    );
    assert!(!page.contains("<style"), "{page}");
    let css = body(&s.request("GET", "/_app/app.css", "", b"")).to_string();
    assert!(
        css.contains(&format!("h1.{class}, .lead.{class} {{")),
        "{css}"
    );
    assert!(css.contains("\n  body {\n    margin: 0"), "{css}");
}

#[test]
fn hooks_and_state() {
    let s = start();
    let home = s.request("GET", "/", "", b"");
    assert_eq!(status(&home), 200);
    assert!(home.contains("<h1>hello from init</h1>"), "{home}");
    assert_eq!(header(&home, "x-app"), Some("test"));

    // What `before` set stays on an error page.
    let missing = s.request("GET", "/nope", "", b"");
    assert_eq!(
        (status(&missing), header(&missing, "x-app")),
        (404, Some("test"))
    );

    // `before` can answer by itself: a CORS preflight, for any path.
    let preflight = s.request("OPTIONS", "/echo", "", b"");
    assert_eq!(
        (
            status(&preflight),
            header(&preflight, "access-control-allow-origin")
        ),
        (204, Some("*"))
    );
}

#[test]
fn components() {
    let s = start();
    let home = s.request("GET", "/", "", b"");
    let cards: Vec<&str> = home
        .split("<section class=\"card\">")
        .skip(1)
        .map(|c| c.split("</section>").next().unwrap())
        .collect();
    assert_eq!(cards.len(), 2, "{home}");
    // Props given, a flag, children with components of their own.
    assert!(
        cards[0].contains("<h2>hello from init ★</h2>") && cards[0].contains("<p>3 items</p>"),
        "{}",
        cards[0]
    );
    assert!(
        cards[0].contains("<span class=\"badge\">new</span>")
            && cards[0].contains("<span class=\"badge\">15</span>"),
        "{}",
        cards[0]
    );
    // Defaults, and no children.
    assert!(
        cards[1].contains("<h2>Plain</h2>")
            && cards[1].contains("<p>0 items</p>")
            && !cards[1].contains("badge"),
        "{}",
        cards[1]
    );
}

#[test]
fn signed_cookies_sign_in() {
    let s = start();
    let away = s.request("GET", "/admin", "", b"");
    assert_eq!(
        (status(&away), header(&away, "location")),
        (303, Some("/login"))
    );

    let login = s.request("POST", "/login", FORM, b"name=ada");
    assert_eq!(
        (status(&login), header(&login, "location")),
        (303, Some("/admin"))
    );
    let cookie = header(&login, "set-cookie").unwrap();
    let value = cookie.split(';').next().unwrap();
    assert!(value.starts_with("user=ada."), "{cookie}");

    let admin = s.request("GET", "/admin", &format!("cookie: {value}\r\n"), b"");
    assert_eq!(status(&admin), 200);
    assert!(admin.contains("Welcome, ada"));
    assert!(
        s.request("GET", "/", &format!("cookie: {value}\r\n"), b"")
            .contains("Signed in as ada")
    );

    // Another name under ada's signature, or no signature, is nobody.
    let mac = value.rsplit('.').next().unwrap();
    for forged in [
        format!("user=bob.{mac}"),
        "user=ada".into(),
        format!("user=ada.{mac}x"),
    ] {
        let r = s.request("GET", "/admin", &format!("cookie: {forged}\r\n"), b"");
        assert_eq!(status(&r), 303, "{forged}");
    }
}

#[test]
fn uploads() {
    let s = start();
    let ct = format!("content-type: {MULTIPART}\r\n");
    let photo = vec![7u8; 2 * 1024 * 1024]; // over the default 1 MB, under the route's 4 MB
    let b = multipart(&[
        ("title", None, b"My cat"),
        ("photo", Some("cat.png"), &photo),
    ]);
    let r = s.request("POST", "/upload", &ct, &b);
    assert_eq!(status(&r), 200, "{}", &r[..r.len().min(300)]);
    assert!(r.contains("cat.png: 2097152 bytes of image/png"), "{r}");

    // No file: the action says so with a 422, and the page keeps the title.
    let b = multipart(&[("title", None, b"My cat")]);
    let r = s.request("POST", "/upload", &ct, &b);
    assert_eq!(status(&r), 422);
    assert!(
        r.contains("Choose a photo") && r.contains("value=\"My cat\""),
        "{r}"
    );

    // An action can answer with a file instead of the page.
    let r = s.request("POST", "/upload?/export", &ct, &b);
    assert_eq!(
        (status(&r), header(&r, "content-type")),
        (200, Some("text/csv"))
    );
    assert_eq!(body(&r), "title\nMy cat\n");

    // Over the route's limit, refused before the body is read.
    let r = s.send(
        format!(
            "POST /upload HTTP/1.1\r\nhost: x\r\n{ct}content-length: {}\r\n\r\n",
            5 * 1024 * 1024
        )
        .as_bytes(),
    );
    assert_eq!(status(&r), 413);
}

#[test]
fn image_uploads_raise_the_body_limit() {
    // `/me` takes a 64 KB picture: its limit is the usual one plus that.
    let s = start_with(&[("WISP_BODY_LIMIT", "16KB")]);
    let ct = format!("content-type: {MULTIPART}\r\n");
    let upload = |target: &str, size: usize| {
        let mut gif = b"GIF89a".to_vec();
        gif.resize(size, 0);
        let b = multipart(&[("avatar", Some("a.gif"), &gif)]);
        status(&s.request("POST", target, &ct, &b))
    };
    assert_eq!(
        upload("/me?/avatar", 60 * 1024),
        303,
        "taken, and sent to sign in"
    );
    assert_eq!(
        upload("/join?/join", 60 * 1024),
        413,
        "a page without uploads"
    );
    assert_eq!(upload("/me?/avatar", 100 * 1024), 413);
}

/// A password hash runs off the worker: with one worker, a page asked for
/// while a sign-up hashes is answered first. A sign-out everywhere ends the
/// sessions made before it, and still does after a restart.
#[test]
fn hashes_leave_the_worker_free_and_sign_outs_last() {
    let dir = Temp::new("sessions");
    let env = [("WISP_THREADS", "1"), ("WISP_DATA", dir.to_str().unwrap())];
    let session = {
        let s = start_with(&env);
        let (home, answered, (joined, hashed)) = std::thread::scope(|t| {
            let joining = t.spawn(|| {
                let form = b"name=ada&password=correct+horse";
                let out = s.request("POST", "/join?/join", FORM, form);
                (out, std::time::Instant::now())
            });
            std::thread::sleep(Duration::from_millis(50));
            let home = s.request("GET", "/", "", b"");
            (home, std::time::Instant::now(), joining.join().unwrap())
        });
        assert_eq!(status(&home), 200);
        assert_eq!(status(&joined), 303, "{joined}");
        assert!(answered < hashed, "the page waited for the hash");

        let set = header(&joined, "set-cookie").unwrap();
        let session = format!("cookie: {}\r\n", set.split(';').next().unwrap());
        assert_eq!(status(&s.request("GET", "/me", &session, b"")), 200);
        let ended = s.request("POST", "/me?/everywhere", &format!("{FORM}{session}"), b"");
        assert_eq!(status(&ended), 303, "{ended}");
        assert_eq!(status(&s.request("GET", "/me", &session, b"")), 303);
        session
    };
    let s = start_with(&env);
    let me = s.request("GET", "/me", &session, b"");
    assert_eq!(
        (status(&me), header(&me, "location")),
        (303, Some("/login"))
    );
}

#[test]
fn body_limits_are_per_route() {
    let s = start();
    let big = vec![b'a'; 100 * 1024];
    assert_eq!(
        status(&s.request("POST", "/echo", "", &big)),
        413,
        "over echo's 64 KB"
    );
    assert_eq!(body(&s.request("POST", "/echo", "", b"hi")), "2:hi");
    // Another route keeps the default 1 MB: this one is refused for its method, not its size.
    assert_eq!(status(&s.request("POST", "/", "", &big)), 405);
}

#[test]
fn chunked_request_bodies() {
    let s = start();
    let r = s.send(b"POST /echo HTTP/1.1\r\nhost: x\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n5\r\nhello\r\n6;ext=1\r\n world\r\n0\r\ntrailer: x\r\n\r\n");
    assert_eq!((status(&r), body(&r)), (200, "11:hello world"));

    // Pipelined after a chunked request, the next is read from where it ended.
    let two = s.send(b"POST /echo HTTP/1.1\r\nhost: x\r\ntransfer-encoding: chunked\r\n\r\n1\r\na\r\n0\r\n\r\nPOST /echo HTTP/1.1\r\nhost: x\r\ncontent-length: 1\r\nconnection: close\r\n\r\nb");
    assert!(two.contains("1:a") && two.contains("1:b"), "{two}");

    let bad: [(&[u8], u16); 5] = [
        (b"5\r\nhel\r\n0\r\n\r\n", 400),    // shorter than it says
        (b"zz\r\nhello\r\n0\r\n\r\n", 400), // not hex
        (b"5\nhello\r\n0\r\n\r\n", 400),    // bare LF
        (b"10001\r\n", 413),                // over echo's 64 KB
        (b"fffffffffffffffffff\r\n", 400),  // more digits than any size has
    ];
    for (chunks, want) in bad {
        let mut raw =
            b"POST /echo HTTP/1.1\r\nhost: x\r\ntransfer-encoding: chunked\r\n\r\n".to_vec();
        raw.extend_from_slice(chunks);
        assert_eq!(
            status(&s.send(&raw)),
            want,
            "{:?}",
            String::from_utf8_lossy(chunks)
        );
    }
    let gzip = s.send(
        b"POST /echo HTTP/1.1\r\nhost: x\r\ntransfer-encoding: gzip, chunked\r\n\r\n0\r\n\r\n",
    );
    assert_eq!(status(&gzip), 501);
    let old = s.send(b"POST /echo HTTP/1.0\r\ntransfer-encoding: chunked\r\n\r\n0\r\n\r\n");
    assert_eq!(status(&old), 400);
}

/// Pipelined in one packet, a route that waits (`/compat` loads with
/// `async fn`) between two that do not: on Linux's epoll the driver answers
/// the first, and the connection's task the rest, in order.
#[test]
fn pipelined_requests_that_wait_and_do_not() {
    let s = start();
    let r = s.send(b"GET / HTTP/1.1\r\nhost: x\r\n\r\nGET /compat?who=ann HTTP/1.1\r\nhost: x\r\n\r\nGET /t/id HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n");
    let at = |part: &str| r.find(part).unwrap_or_else(|| panic!("{part} in {r}"));
    assert_eq!(r.matches("HTTP/1.1 200 OK").count(), 3, "{r}");
    assert!(
        at("hello from init") < at("ann") && at("ann") < r.rfind("HTTP/1.1 200").unwrap(),
        "{r}"
    );
}

#[test]
fn server_sent_events() {
    let s = start();
    let r = s.send(b"GET /events HTTP/1.1\r\nhost: x\r\n\r\nGET / HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n");
    assert_eq!(status(&r), 200);
    assert_eq!(header(&r, "content-type"), Some("text/event-stream"));
    assert_eq!(header(&r, "transfer-encoding"), Some("chunked"));
    assert_eq!(header(&r, "cache-control"), Some("no-store"));
    for i in 0..3 {
        assert!(
            r.contains(&format!("data: tick {i}\ndata: line two\n\n")),
            "{r}"
        );
    }
    // The stream ended properly, and the connection went on to the next request.
    let (events, next) = r.split_once("0\r\n\r\n").unwrap();
    assert!(!events.is_empty());
    assert!(
        next.starts_with("HTTP/1.1 200 OK") && next.contains("hello from init"),
        "{next}"
    );

    // HTTP/1.0 has no chunks: the body runs to the end of the connection.
    let old = s.send(b"GET /events HTTP/1.0\r\n\r\n");
    assert_eq!(
        (
            header(&old, "transfer-encoding"),
            header(&old, "connection")
        ),
        (None, Some("close"))
    );
    assert!(old.ends_with("data: tick 2\ndata: line two\n\n"), "{old}");
}

const UPGRADE: &str = "upgrade: websocket\r\nconnection: Upgrade\r\nsec-websocket-version: 13\r\nsec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\n";

#[test]
fn websocket_idle() {
    let s = start();
    let mut c = connect(s.port);
    c.write_all(format!("GET /ws HTTP/1.1\r\nhost: x\r\n{UPGRADE}\r\n").as_bytes())
        .unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut b = [0u8];
        c.read_exact(&mut b).unwrap();
        head.push(b[0]);
    }
    // Quiet for half of WISP_WS_IDLE: pinged. The pong counts as traffic.
    let t = std::time::Instant::now();
    assert_eq!(ws::read(&mut c), (0x89, Vec::new()));
    c.write_all(&ws::frame(0x8a, b"")).unwrap();
    assert_eq!(ws::read(&mut c), (0x89, Vec::new()));
    // Then silent for all of it: closed with 1001.
    assert_eq!(ws::read(&mut c), (0x88, 1001u16.to_be_bytes().to_vec()));
    let waited = t.elapsed().as_secs_f64();
    assert!((2.5..3.8).contains(&waited), "{waited}"); // ping at 1, pong, ping at 2, close at 3
}

#[test]
fn websocket_echo() {
    let s = start();
    let mut c = connect(s.port);
    // The first message rides in the same packet as the handshake.
    let mut hello = format!(
        "GET /ws HTTP/1.1\r\nhost: 127.0.0.1:{}\r\norigin: http://127.0.0.1:{}\r\n{UPGRADE}\r\n",
        s.port, s.port
    )
    .into_bytes();
    hello.extend(ws::frame(0x81, b"first"));
    c.write_all(&hello).unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut b = [0u8];
        c.read_exact(&mut b).unwrap();
        head.push(b[0]);
    }
    let head = String::from_utf8(head).unwrap();
    assert_eq!(status(&head), 101, "{head}");
    assert_eq!(
        header(&head, "sec-websocket-accept"),
        Some("s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")
    );
    assert_eq!(header(&head, "upgrade"), Some("websocket"));
    assert_eq!(header(&head, "x-app"), Some("test"), "hooks still run");
    assert_eq!(ws::read(&mut c), (0x81, b"first".to_vec()));

    // A fragmented text message with a ping between its parts, then binary.
    let mut more = ws::frame(0x01, b"frag");
    more.extend(ws::frame(0x89, b"are you there"));
    more.extend(ws::frame(0x80, b"mented"));
    more.extend(ws::frame(0x82, &[0xab; 300]));
    c.write_all(&more).unwrap();
    assert_eq!(ws::read(&mut c), (0x8a, b"are you there".to_vec()));
    assert_eq!(ws::read(&mut c), (0x81, b"fragmented".to_vec()));
    assert_eq!(ws::read(&mut c), (0x82, vec![0xab; 300]));

    // A close is echoed, and the connection ends.
    c.write_all(&ws::frame(0x88, &1000u16.to_be_bytes()))
        .unwrap();
    assert_eq!(ws::read(&mut c), (0x88, 1000u16.to_be_bytes().to_vec()));
    let mut rest = Vec::new();
    assert_eq!(c.read_to_end(&mut rest).map(|_| rest.len()).unwrap_or(0), 0);

    // An unmasked frame breaks the protocol: closed with 1002.
    let mut c = connect(s.port);
    let mut raw = format!("GET /ws HTTP/1.1\r\nhost: x\r\n{UPGRADE}\r\n").into_bytes();
    raw.extend([0x81, 0x02, b'h', b'i']);
    c.write_all(&raw).unwrap();
    let mut got = Vec::new();
    let _ = c.read_to_end(&mut got);
    assert!(got.ends_with(&[0x88, 0x02, 0x03, 0xea]), "{got:?}");

    // Refused: another site's page (as a cross-site form post would be), a
    // plain request, another version.
    let other = s.request(
        "GET",
        "/ws",
        &format!("origin: https://evil.example\r\n{UPGRADE}"),
        b"",
    );
    assert_eq!(status(&other), 403, "{other}");
    let plain = s.request("GET", "/ws", "", b"");
    assert_eq!(
        (status(&plain), header(&plain, "upgrade")),
        (426, Some("websocket"))
    );
    let old = s.request(
        "GET",
        "/ws",
        &UPGRADE.replace("version: 13", "version: 8"),
        b"",
    );
    assert_eq!(
        (status(&old), header(&old, "sec-websocket-version")),
        (426, Some("13"))
    );
}

#[test]
fn files_from_a_directory() {
    let s = start();
    let card = s.request("GET", "/files/Card.wisp", "", b"");
    assert_eq!(status(&card), 200);
    assert!(body(&card).starts_with("{@props title: &str"), "{card}");
    for outside in [
        "/files/..%2FCargo.toml",
        "/files/%2E%2E/Cargo.toml",
        "/files/C:%5Cwindows",
        "/files/nope.wisp",
        "/files/",
    ] {
        let r = s.request("GET", outside, "", b"");
        assert!(!r.contains("[package]"), "{outside}");
        assert!(matches!(status(&r), 404 | 308), "{outside}: {r}");
    }
}

#[test]
fn browser_code() {
    let s = start();
    let page = s.request("GET", "/live", "", b"");
    assert_eq!(status(&page), 200, "{page}");

    // Directives leave the HTML; each element is marked with its instance
    // (the page is 0, the component in it 1) and its group.
    assert!(
        !page.contains("on:click") && !page.contains(":hidden") && !page.contains("bind:"),
        "{page}"
    );
    assert!(
        page.contains("<h1 data-w=\"0.0\">Shop &amp; &lt;save&gt;</h1>"),
        "{page}"
    );
    assert!(
        page.contains("<button data-w=\"0.1\">Menu</button>"),
        "{page}"
    );
    // `:hidden="!open"`, with `let open = false`: hidden from the start.
    assert!(page.contains("<ul data-w=\"0.2\" hidden>"), "{page}");
    assert!(page.contains("<input data-w=\"0.4\">"), "{page}");
    // A Rust loop's values that a directive reads, as HTML-escaped JSON.
    assert!(page.contains("<li data-w=\"0.3\" data-wl=\"{&quot;item&quot;:{&quot;name&quot;:&quot;tea&quot;}}\">0: tea</li>"), "{page}");
    assert!(page.contains("data-wl=\"{&quot;item&quot;:{&quot;name&quot;:&quot;cake \\&quot;big\\&quot;&quot;}}\">1: cake &quot;big&quot;</li>"), "{page}");
    // A client `<template each>`: the elements inside have no instance.
    assert!(
        page.contains("<template data-w=\"0.6\"><p data-w=\"7\"></p></template>"),
        "{page}"
    );
    assert!(
        page.contains("<button data-w=\"1.0\">More</button>")
            && page.contains("<div data-w=\"1.1\" hidden>Menu</div>"),
        "{page}"
    );
    // The client script is not in the page.
    assert!(!page.contains("let open"), "{page}");

    // The instances and their modules end the body.
    let json_at = page
        .find("<script type=\"application/json\" id=\"wisp-live\">")
        .expect("the instances");
    assert!(
        json_at < page.find("</body>").unwrap() && json_at > page.find("More</button>").unwrap()
    );
    let json = &page[json_at..];
    let json = &json[json.find('>').unwrap() + 1..json.find("</script>").unwrap()];
    let modules: Vec<(&str, &str)> = json["{\"m\":{".len()..json.find('}').unwrap()]
        .split(',')
        .map(|kv| {
            kv.split_once(':')
                .map(|(k, v)| (k.trim_matches('"'), v.trim_matches('"')))
                .unwrap()
        })
        .collect();
    assert_eq!(modules.len(), 2, "{json}");
    let (page_id, page_url) = modules[0];
    let (comp_id, comp_url) = modules[1];
    assert!(
        page_url.starts_with(&format!("/_app/c/{page_id}.js?v=")),
        "{json}"
    );
    assert!(
        json.ends_with(&format!(
            "\"i\":[[0,\"{page_id}\",-1,{{\"data\":{{\"title\":\"Shop \\u0026 \\u003csave\\u003e\"}}}}],[1,\"{comp_id}\",0,{{\"label\":\"More\"}}]]}}"
        )),
        "{json}"
    );
    for url in [page_url, comp_url] {
        assert!(
            page.contains(&format!("<link rel=\"modulepreload\" href=\"{url}\">")),
            "{page}"
        );
    }
    let runtime = format!(
        "<script type=\"module\" src=\"/_app/live.js?v={}\"></script>",
        wisp::rt::RUNTIME_VERSION
    );
    assert!(page.contains(&format!("{runtime}\n</body>")), "{page}");

    // The modules, and the runtime they import.
    let module = s.request("GET", page_url, "", b"");
    assert_eq!(
        (status(&module), header(&module, "content-type")),
        (200, Some("text/javascript; charset=utf-8"))
    );
    assert_eq!(
        header(&module, "cache-control"),
        Some("public, max-age=31536000, immutable")
    );
    let js = body(&module);
    assert!(js.starts_with(&format!("import {{ define }} from \"/_app/live.js?v={}\";\ndefine(\"{page_id}\", function (__wisp_p, __wisp_h) {{ const {{", wisp::rt::RUNTIME_VERSION)), "{js}");
    assert!(
        js.contains("const { data } = __wisp_props(__wisp_p, [\"data\"]);"),
        "{js}"
    );
    assert!(
        js.contains("[\"on\", \"click\", 8193, ({ item }, event) => (pick(item.name))]"),
        "{js}"
    );
    assert!(
        js.contains("[\"each\", () => (notes), [\"note\", \"n\"], null]"),
        "{js}"
    );
    // In dev it names its source map, served beside it, which names the file.
    assert!(
        js.ends_with(&format!("//# sourceMappingURL={page_id}.js.map\n")),
        "{js}"
    );
    let map = s.request("GET", &format!("/_app/c/{page_id}.js.map"), "", b"");
    assert_eq!(header(&map, "content-type"), Some("application/json"));
    assert!(
        body(&map).contains("\"sources\":[\"wisp:///src/routes/live/+page.wisp\"]"),
        "{}",
        body(&map)
    );
    // The script keeps its line numbers: `let open` is on line 14 of the
    // file. It is state: a signal.
    assert_eq!(
        js.lines()
            .position(|l| l.trim() == "let open = __wisp_s(false)"),
        Some(13),
        "{js}"
    );
    let etag = header(&module, "etag").unwrap().to_string();
    assert_eq!(
        status(&s.request("GET", page_url, &format!("if-none-match: {etag}\r\n"), b"")),
        304
    );
    assert!(body(&s.request("GET", comp_url, "", b"")).contains("define(\""));
    let live = s.request(
        "GET",
        &format!("/_app/live.js?v={}", wisp::rt::RUNTIME_VERSION),
        "",
        b"",
    );
    assert_eq!(
        (status(&live), header(&live, "content-type")),
        (200, Some("text/javascript; charset=utf-8"))
    );
    assert!(body(&live).contains("export function define"));
    assert_eq!(status(&s.request("GET", "/_app/c/t999.js", "", b"")), 404);

    // A page without browser code has none of this.
    assert!(!s.request("GET", "/", "", b"").contains("wisp-live"));
}

#[test]
fn form_origin_behind_a_proxy() {
    let s = start();
    // A proxy that sends its own Host and the visitor's in X-Forwarded-Host.
    let proxied = format!("{FORM}origin: https://example.com\r\nx-forwarded-host: example.com\r\n");
    let r = s.request("POST", "/login", &proxied, b"name=ada");
    assert_eq!(status(&r), 303);
    let foreign =
        format!("{FORM}origin: https://evil.example\r\nx-forwarded-host: example.com\r\n");
    assert_eq!(
        status(&s.request("POST", "/login", &foreign, b"name=ada")),
        403
    );
}

/// The `n`th module URL in a page's `#wisp-live` JSON.
fn module_url(page: &str, n: usize) -> String {
    let json = &page[page.find("id=\"wisp-live\">").expect("instances")..];
    let url = json.split("\"/_app/c/").nth(n + 1).expect("a module");
    format!("/_app/c/{}", &url[..url.find('"').unwrap()])
}

#[test]
fn client_parity() {
    let s = start();
    let v = wisp::rt::RUNTIME_VERSION;

    // `{:expr}`: an anchor, what the server knows of it (a server value's
    // path, or a script variable set to a literal), and an end. A live
    // attribute keeps its static text. Client blocks are templates whose
    // elements bind per copy, and the copies the server could work out.
    let holes = s.request("GET", "/a2/holes", "", b"");
    assert!(
        holes.contains("<h1 id=\"title\"><template data-w=\"0.0\"></template>Holes<!----></h1>"),
        "{holes}"
    );
    assert!(
        holes.contains("</template><!--[--><li data-w=\"9\" data-id=\"1\"><template data-w=\"10\"></template>0<!---->: <template data-w=\"11\"></template>one<!----></li>")
            && holes.contains("<!--[--><p id=\"named\">Named <template data-w=\"15\"></template>Ann<!----></p>")
            && !holes.contains("<!--[--><p id=\"closed\">"),
        "{holes}"
    );
    assert!(
        holes.contains("<template data-w=\"0.17\"></template>Hello &lt;server&gt;<!----></p>"),
        "{holes}"
    );
    assert!(
        holes.contains("<p id=\"greet\" class=\"card \" data-w=\"0.1\">"),
        "{holes}"
    );
    assert!(
        holes.contains(
            "<template data-w=\"0.8\"><li data-w=\"9\"><template data-w=\"10\"></template><!---->: "
        ),
        "{holes}"
    );
    let js = body(&s.request("GET", &module_url(&holes, 0), "", b"")).to_string();
    for want in [
        // Never written: constants, not signals.
        "[\"hole\", () => (title)]",
        "[\"attr\", \"class\", () => (`card ${(mood) ?? ''}`)]",
        "[\"attr\", \"aria-expanded\", () => (open.v)]",
        "[\"each\", () => (items.v), [\"item\", \"i\"], ({ item, i }) => (item.id)]",
        "[\"animate\", \"flip\", null]",
        "[\"if\", () => (![...(items.v ?? [])].length)]",
        "[\"if\", () => (!(open.v) && (name.v))]",
        "[\"if\", () => (!(open.v) && !(name.v))]",
    ] {
        assert!(js.contains(want), "{want} in {js}");
    }

    // Instances record the one they render inside, for context.
    let state = s.request("GET", "/a2/state", "", b"");
    assert!(
        state.contains(",0,{}],[2,\"") && state.contains("<div id=\"fresh\" data-wisp-reset>"),
        "{state}"
    );
    // State is signals, read as `.v`; `$derived` is a memo; `watch` is a
    // helper.
    let js = body(&s.request("GET", &module_url(&state, 0), "", b"")).to_string();
    assert!(
        js.contains("let double = __wisp_d(() => (clicks.v * 2))")
            && js.contains("watch(() => data.v.count, () => bumps.v++)")
            && js.contains(" effect, watch, derived, "),
        "{js}"
    );

    // A component the browser renders: the page's module imports its
    // module, which carries its markup, slot and all.
    let comps = s.request("GET", "/a2/comps", "", b"");
    let js = body(&s.request("GET", &module_url(&comps, 0), "", b"")).to_string();
    assert!(js.contains("[[\"count\", ({ name }) => (counts.v[name]), ({ name }, __wisp_v) => { (counts.v[name]) = __wisp_v }]], [[\"bump\", (_, event) => (bumped.v = event)]]]"), "{js}");
    let item = js
        .lines()
        .find_map(|l| l.strip_prefix("import \""))
        .map(|l| l.trim_end_matches("\";"))
        .expect("the component's module");
    let item = body(&s.request("GET", item, "", b"")).to_string();
    assert!(
        item.contains("const { label, count } = __wisp_props(__wisp_p, [\"label\", \"count\"]);")
            && item.contains("<template data-wslot></template>")
            && item.contains("{ html: \""),
        "{item}"
    );

    // `$lib` files: one hash for all, imports rewritten to the same URLs.
    let stores = s.request("GET", "/a2/stores", "", b"");
    let js = body(&s.request("GET", &module_url(&stores, 0), "", b"")).to_string();
    let lib = js
        .split("from \"")
        .nth(2)
        .and_then(|r| r.split('"').next())
        .expect("the lib import")
        .to_string();
    assert!(lib.starts_with("/_app/c/lib/cart.js?v="), "{js}");
    let cart = s.request("GET", &lib, "", b"");
    assert_eq!(
        header(&cart, "cache-control"),
        Some("public, max-age=31536000, immutable")
    );
    let hash = &lib[lib.find("?v=").unwrap()..];
    assert!(
        body(&cart).contains(&format!("from \"/_app/live.js?v={v}\""))
            && body(&cart).contains(&format!("from \"/_app/c/lib/names.js{hash}\"")),
        "{cart}"
    );

    // `+page.js`: the page gets all of `data` for its `load`.
    let load = s.request("GET", "/a2/load", "", b"");
    assert!(
        load.contains("{\"data\":{\"server\":\"server\"}}"),
        "{load}"
    );
    let js = body(&s.request("GET", &module_url(&load, 0), "", b"")).to_string();
    let url = js
        .split("import * as __wisp_u from \"")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .expect("the load module")
        .to_string();
    assert!(js.contains("{ load: __wisp_u.load"), "{js}");
    assert!(body(&s.request("GET", &url, "", b"")).contains("export async function load"));

    // Browser code that fails to start asks for the route's error page.
    let err = s.request(
        "GET",
        "/a2/holes",
        "x-wisp-error: 1\r\naccept: text/html\r\n",
        b"",
    );
    assert_eq!(status(&err), 500);
    assert!(err.contains("<h1 id=\"err\">Error 500</h1>"), "{err}");
}

#[test]
fn blocks_elements_and_props() {
    let s = start();
    let page = s.request("GET", "/a2/more", "", b"");
    // The first paint: {:#key}, an {:#await}'s pending branch, a {:#try}'s
    // body, <wisp:element>'s tag, `class={:[…]}`, `style={:{…}}`, a
    // spread, and a component whose props only `$props()` names (renamed,
    // and the rest).
    for want in [
        "<!--[--><p id=\"keyed\">v<template data-w=\"6\"></template>1<!----></p><!--]-->",
        "<!--[--><p id=\"wait\">Loading</p>",
        "<!--[--><p id=\"tried\">",
        "<h2 id=\"dyn\" data-w=\"0.36\">dynamic</h2>",
        "<span  data-w=\"1.0\" title=\"tip\" class=\"pill warm\"><template data-w=\"1.1\"></template>served<!----></span>",
        "data-x=\"1\" title=\"spread\" style=\"color:red;font-weight:700\">styled</p>",
        // <wisp:window /> is a template whose directives go on the window.
        "<template  data-w=\"0.0\"></template>",
    ] {
        assert!(page.contains(want), "{want} in {page}");
    }
    // The runtime's less used half comes with the modules that use it.
    let extra = page
        .split("<link rel=\"modulepreload\" href=\"")
        .find_map(|p| p.strip_prefix("/_app/c/extra.js"))
        .map(|p| format!("/_app/c/extra.js{}", &p[..p.find('"').unwrap()]))
        .expect("extra.js preloaded");
    let js = s.request("GET", &extra, "", b"");
    assert!(
        status(&js) == 200
            && body(&js).contains("X.await")
            && body(&js).contains("from \"/_app/live.js?v="),
        "{js}"
    );
    let module = body(&s.request("GET", &module_url(&page, 0), "", b"")).to_string();
    for want in [
        "import \"/_app/c/extra.js?v=",
        "[\"at\", \"window\"], [\"bind\", \"innerWidth\"",
        "[\"key\", () => (version.v)]",
        "[\"await\", () => (slow.v)]",
        "[\"try\"]",
        "[\"transition\", \"fade\", null, 1], [\"transition\", () => spin, null, 2]",
        "[\"wait\", \"x\"]",
        "[\"spread\", () => (attrs.v)]",
        "[\"tag\", () => (tag.v)]",
    ] {
        assert!(module.contains(want), "{want} in {module}");
    }
    // A page with none of it does not load it.
    assert!(!s.request("GET", "/a2/state", "", b"").contains("extra.js"));
}

#[test]
fn spreads_resets_and_rust_reads_of_props() {
    let s = start();
    let page = s.request("GET", "/a2/props", "", b"");
    // A prop only `$props()` names shows in Rust as in the browser.
    assert!(
        page.contains("<b class=\"chip\" data-label=\"rust\">rust: <template data-w=\"1.0\"></template>1<!----></b>"),
        "{page}"
    );
    let module = body(&s.request("GET", &module_url(&page, 0), "", b"")).to_string();
    for want in [
        // Spread props, where they stand among the others.
        "({ p }) => ({ ...(p), \"on\": (p.label === picked) })",
        "() => ({ ...(one.v), \"title\": \"after\" })",
        // `reset` in a try's `{:catch}`.
        "({ reset }, event) => { broken.v = false; reset() }",
    ] {
        assert!(module.contains(want), "{want} in {module}");
    }
}

#[test]
fn islands_and_runes() {
    let s = start();
    let v = wisp::rt::RUNTIME_VERSION;
    // Every instance is an island: the page is painted whole, and nothing
    // loads until an island's moment comes (wisp.js wakes them). A
    // `client:none` one sends neither its values nor its module.
    let page = s.request("GET", "/a2/islands", "", b"");
    assert!(
        page.contains("<button class=\"tally\" data-w=\"4.0\"><template data-w=\"4.1\"></template>0<!----></button>"),
        "{page}"
    );
    let json = &page[page.find("id=\"wisp-live\">").expect("instances")..];
    assert!(
        json.contains("\"i\":[[0,\"t")
            && json.contains(",-1,{},\"i\"],[1,")
            && json.contains(",-1,{},\"m(min-width: 1px)\"],[2,")
            && json.contains(",-1,{\"value\":0,\"step\":1},\"x\"],[4,")
            && json.contains(",-1,{},\"v\"]]}"),
        "{json}"
    );
    assert!(!json.contains("[3,"), "{json}");
    assert!(
        !page.contains("modulepreload") && !page.contains(&format!("/_app/live.js?v={v}")),
        "{page}"
    );
    // wisp.js has the wake-up, and live.js the hydrate() it calls.
    assert!(body(&s.request("GET", "/_app/wisp.js", "", b"")).contains("m.hydrate(I)"));
    assert!(
        body(&s.request("GET", &format!("/_app/live.js?v={v}"), "", b""))
            .contains("export function hydrate")
    );

    // Runes compile to signals, memos and effects; `$store` reads the store.
    let runes = s.request("GET", "/a2/runes", "", b"");
    assert!(runes.contains(&format!("/_app/live.js?v={v}")), "{runes}");
    let js = body(&s.request("GET", &module_url(&runes, 0), "", b"")).to_string();
    for want in [
        "let count = __wisp_s(0)",
        "let double = __wisp_d(() => (count.v * 2))",
        "let done = __wisp_d(() => (todos.v.filter((t) => t.done).length))",
        "__wisp_e(() => {\n    document.title = `Runes ${count.v}`",
        "__wisp_e(() => console.log(...[count.v].map(__wisp_snap)))",
        "[\"hole\", () => (cart.value.length)]",
        "(_, event) => (todos.v.push({ id: todos.v.length + 1, text: 'new', done: false }))",
    ] {
        assert!(js.contains(want), "{want} in {js}");
    }
    let stepper = js
        .lines()
        .find_map(|l| l.strip_prefix("import \""))
        .map(|l| l.trim_end_matches("\";"))
        .expect("the component's module");
    let stepper = body(&s.request("GET", stepper, "", b"")).to_string();
    assert!(
        stepper.contains(
            "const { value, step } = __wisp_props(__wisp_p, [\"value\", \"step\"], { value: () => (0), step: () => (1) });"
        ) && stepper.contains("(_, event) => (value.v += step.v)"),
        "{stepper}"
    );
}

#[test]
fn first_paint() {
    let s = start();
    let page = s.request("GET", "/a2/paint", "", b"");
    let has = |want: &str| assert!(page.contains(want), "{want} in {page}");
    // Script variables set from `data`, a keyed each with a nested if, an
    // each's {:else}, and Rust loop values: painted after their templates.
    has("<template data-w=\"0.0\"></template>Paint &lt;me&gt;<!---->");
    has(
        "<!--[--><li class=\"todo\" data-w=\"2\"><template data-w=\"3\"></template>0<!---->. <template data-w=\"4\"></template>one<!---->",
    );
    has("</template><!--[--> <b class=\"tick\">done</b><!--]--></li>");
    has("<!--[--><li id=\"empty-else\">empty</li><!--]-->");
    // The if's template in the each's template and in both copies, and
    // the one copy of it whose todo is done.
    assert_eq!(page.matches("<b class=\"tick\">").count(), 4, "{page}");
    assert_eq!(
        page.matches("<span class=\"rdone\">one</span>").count(),
        2,
        "{page}"
    );
    assert_eq!(
        page.matches("<span class=\"rdone\">two</span>").count(),
        1,
        "{page}"
    );
    // A component that renders itself, painted to the end of its data.
    let names: Vec<&str> = page
        .split("<li class=\"node\"><template data-w=\"0\"></template>")
        .skip(1)
        .map(|s| &s[..s.find('<').unwrap()])
        .collect();
    assert_eq!(names, ["root", "a", "a1", "b"]);
    // A component with its props and slot.
    has("<span class=\"label\"><template data-w=\"0\"></template>one<!----></span>");
    has(
        "<template data-wslot></template><!--[--><i class=\"slot\"><template data-w=\"13\"></template>1<!----></i><!--]-->",
    );
    // A boolean attribute the browser sets from a literal script value is
    // there from the start; one the server cannot work out is left to it.
    has("<div id=\"menu\" data-w=\"");
    assert!(page.contains("\" hidden>menu</div>"), "{page}");
    assert!(page.contains("<details id=\"more\" data-w=\""), "{page}");
    assert!(
        !page.contains("\" open>x</details>") && !page.contains("hidden>x</details>"),
        "{page}"
    );
    // `data` in an arrow's or a function's parameters is not the page's:
    // only the fields the page reads are sent.
    assert!(!page.contains("secret"), "{page}");

    // The recursive component's module needs no import of itself.
    let js = body(&s.request("GET", &module_url(&page, 0), "", b"")).to_string();
    let tree = js
        .lines()
        .filter_map(|l| l.strip_prefix("import \"")?.strip_suffix("\";"))
        .find(|u| {
            let map = format!("{}.map", &u[..u.find('?').unwrap()]);
            body(&s.request("GET", &map, "", b"")).contains("Tree.wisp")
        })
        .expect("the tree's module")
        .to_string();
    let src = body(&s.request("GET", &tree, "", b"")).to_string();
    assert!(
        !src.contains(&tree[..tree.find('?').unwrap()]) && src.contains("[\"comp\", "),
        "{src}"
    );

    // A `+page.js` load gets the route and its parameters.
    let params = s.request("GET", "/a2/params/one", "", b"");
    assert!(
        params.contains("]],\"r\":\"/a2/params/[slug]\",\"p\":{\"slug\":\"one\"}}</script>"),
        "{params}"
    );
}

#[test]
fn template_shorthands() {
    let s = start();
    let page = s.request("GET", "/sugar", "", b"");
    let html = body(&page);
    // `class:` joins the static classes, the shorthand is `href={href}`, a
    // `None` leaves its attribute out, a URL is still guarded.
    for want in [
        "<h1 class=\"big on\">a&lt;b</h1>",
        "<p class=\"on\">7</p>",
        "<a href=\"/x?a=1&amp;b=2\" title=\"hi &quot;you&quot;\" class=\"link a&lt;b\" hidden>link</a>",
        "<a href=\"about:invalid#blocked\" rel=\"x\">bad</a>",
    ] {
        assert!(html.contains(want), "{want}\n{html}");
    }
    let api = s.request("GET", "/sugar/api", "", b"");
    assert_eq!(header(&api, "content-type"), Some("application/json"));
    assert_eq!(body(&api), "[[\"a\\u003cb\",1],[\"c\",null]]");
}

#[test]
fn inputs_by_name() {
    let s = start();
    // A route parameter, then query values: `Option`, `Vec` and a checkbox.
    let r = s.request("GET", "/inputs/7?q=tea&n=1&n=2&on=on", "", b"");
    assert_eq!(header(&r, "content-type"), Some("application/json"));
    assert_eq!(
        body(&r),
        "{\"id\":7,\"q\":\"tea\",\"n\":[1,2],\"on\":true,\"names\":[]}"
    );
    // A path that is not one of these is no page; a bad value is a 400.
    assert_eq!(status(&s.request("GET", "/inputs/x", "", b"")), 404);
    let bad = s.request("GET", "/inputs/7?n=x", "", b"");
    assert_eq!(status(&bad), 400);
    // A form's fields; an endpoint that returns nothing answers 204.
    assert_eq!(
        status(&s.request("POST", "/inputs/7", FORM, b"name=ada")),
        204
    );
    let missing = s.request("POST", "/inputs/7", FORM, b"");
    assert!(
        status(&missing) == 400 && missing.contains("missing form field `name`"),
        "{missing}"
    );
    assert_eq!(
        body(&s.request("GET", "/inputs/7", "", b"")),
        "{\"id\":7,\"q\":null,\"n\":[],\"on\":false,\"names\":[\"ada\"]}"
    );
}

/// A page whose Rust is its `---` block: statements as the load, an action
/// that returns `invalid`, `cx` and a module of the app's own in markup.
#[test]
fn page_blocks() {
    let s = start();
    let page = s.request("GET", "/signup", "", b"");
    assert!(
        status(&page) == 200
            && page.contains("<title>Sign up</title>")
            && page.contains("<p id=\"price\">300 at /signup</p>")
            && !page.contains("problem"),
        "{page}"
    );
    // A problem shows the page again, as a 422, with what was typed.
    let bad = s.request("POST", "/signup", FORM, b"email=ada");
    assert!(
        status(&bad) == 422
            && bad.contains("value=\"ada\"")
            && bad.contains("<p class=\"problem\">needs an @</p>"),
        "{bad}"
    );
    assert_eq!(
        status(&s.request("POST", "/signup", FORM, b"email=a@b")),
        303
    );
    // Route parameters are locals, with no Rust at all.
    assert!(body(&s.request("GET", "/post/hi", "", b"")).contains("<h1>Post hi</h1>"));
}

#[test]
fn param_matchers() {
    let s = start();
    let m = |path: &str| {
        let r = s.request("GET", path, "", b"");
        match body(&r).split_once("<p id=\"m\">") {
            Some((_, rest)) => rest.split("</p>").next().unwrap().to_string(),
            None => status(&r).to_string(),
        }
    };
    // A matched param goes before an unmatched one; a segment a matcher
    // refuses goes on to the next route. Matchers see the decoded segment.
    assert_eq!(m("/match/12"), "int 12");
    assert_eq!(m("/match/ab"), "word ab");
    assert_eq!(m("/match/%61b"), "word ab");
    assert_eq!(m("/match/Zz"), "slug Zz");
    // `int` takes only what fits a u64, so its `parse().unwrap()` holds.
    assert_eq!(
        m("/match/99999999999999999999"),
        "slug 99999999999999999999"
    );
    assert_eq!(m("/match/opt"), "opt");
    assert_eq!(m("/match/opt/5"), "opt");
    assert_eq!(m("/match/opt/x"), "404");
}

#[test]
fn snippets() {
    let s = start();
    let page = s.request("GET", "/snippets", "", b"");
    let html = body(&page);
    // Rendered in the page, with a snippet rendering another.
    let rows = "<tr><td>0</td><td>pen</td><td>2</td></tr><tr><td>1</td><td>ink</td><td>5</td></tr>";
    assert!(
        html.contains(&format!("<table id=\"own\">{rows}</table>")),
        "{html}"
    );
    // Given to a component, by name and as a child next to its children.
    assert!(
        html.contains(&format!("<table class=\"t\">{rows}</table>")),
        "{html}"
    );
    assert!(
        html.contains(
            "<table class=\"t\"><tr><td>0:pen</td></tr><tr><td>1:ink</td></tr>

<caption>kids</caption>
</table>"
        ),
        "{html}"
    );
    // `{:@render}`: drawn by the browser, and first by the server when it
    // knows the arguments.
    let chips = html.split("<p id=\"chips\">").nth(1).unwrap();
    let chips = chips.split("</p>").next().unwrap();
    assert_eq!(chips.matches("<b class=\"chip\">").count(), 5, "{chips}");
    assert!(
        chips.contains("<b class=\"chip\"><template data-w=\"3\"></template>x<!----></b>"),
        "{chips}"
    );
    assert!(chips.contains(">y<!----></b>"), "{chips}");
}

#[test]
fn connection_cap() {
    let s = start_with(&[("WISP_MAX_CONNS", "2")]);
    let open = || connect(s.port);
    // Two idle connections fill it (one of them a WebSocket), so a third
    // is answered 503 and closed without being read. Each is answered
    // first: one only connected may not be accepted yet (io_uring's workers
    // each accept from a listener of their own), so it holds no slot.
    let mut head = [0u8; 12];
    let mut a = open();
    write!(a, "GET / HTTP/1.1\r\nhost: x\r\n\r\n").unwrap();
    a.read_exact(&mut head).unwrap();
    assert_eq!(&head, b"HTTP/1.1 200");
    let mut ws = open();
    write!(ws, "GET /ws HTTP/1.1\r\nhost: x\r\n{UPGRADE}\r\n").unwrap();
    ws.read_exact(&mut head).unwrap();
    assert_eq!(&head, b"HTTP/1.1 101");
    let mut got = String::new();
    let _ = open().read_to_string(&mut got);
    assert_eq!(status(&got), 503, "{got}");
    // A slot comes back when a connection closes.
    drop(a);
    for _ in 0..50 {
        if status(&s.request("GET", "/", "", b"")) == 200 {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("no slot came back");
}

#[test]
fn long_form_route_files() {
    let s = start();
    // Docs, `#![…]`, explicit imports and `pub` are all still allowed.
    assert!(body(&s.request("GET", "/compat?who=you", "", b"")).contains("hi you"));
    assert_eq!(status(&s.request("GET", "/compat", "", b"")), 400);
}

/// A second secret, for signed cookies across a change of `WISP_SECRET`.
const NEW: &str = "fedcba9876543210fedcba9876543210";

/// The `user=…` cookie the login form sets.
fn sign_in(secret: &str) -> String {
    let s = start_with(&[("WISP_SECRET", secret)]);
    let r = s.request("POST", "/login", FORM, b"name=ada");
    let set = header(&r, "set-cookie").unwrap_or_else(|| panic!("no cookie in {r}"));
    set.split(';').next().unwrap().to_string()
}

/// The status of `/admin`, which only a signed-in visitor sees.
fn admin(env: &[(&str, &str)], cookie: &str) -> u16 {
    let s = start_with(env);
    status(&s.request("GET", "/admin", &format!("cookie: {cookie}\r\n"), b""))
}

/// With the old secret in `WISP_SECRET_OLD`, cookies it signed still hold;
/// without, they do not.
#[test]
fn an_old_secret_keeps_cookies_it_signed() {
    let cookie = sign_in(SECRET);
    assert!(cookie.starts_with("user="), "{cookie}");
    assert_eq!(admin(&[("WISP_SECRET", SECRET)], &cookie), 200);
    let both = [("WISP_SECRET", NEW), ("WISP_SECRET_OLD", SECRET)];
    assert_eq!(admin(&both, &cookie), 200);
    assert_eq!(admin(&[("WISP_SECRET", NEW)], &cookie), 303, "signed out");
    // A cookie the new secret signs holds with it alone, as always.
    assert_eq!(admin(&both, &sign_in(NEW)), 200);
}

/// An old secret too short to be safe stops the server, as a short
/// `WISP_SECRET` does.
#[test]
fn a_short_old_secret_stops_the_server() {
    let out = common::command(&[("WISP_SECRET_OLD", "short")])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("WISP_SECRET_OLD is 5 characters"), "{said}");
}

#[test]
fn after_and_report_hooks() {
    let s = start();
    let home = s.request("GET", "/", "", b"");
    assert_eq!(header(&home, "x-reports"), Some("0 so far"), "{home}");
    assert_eq!(status(&s.request("GET", "/boom", "", b"")), 500);
    let after = s.request("GET", "/nope", "", b"");
    assert_eq!(header(&after, "x-reports"), Some("1 so far"), "{after}");
}

/// `{#await}`: the page goes out at once, each pending markup in place, and
/// each answer follows in the same chunked response as it comes; an error is
/// its `{:catch}`, a panic the default text. A page without one is sent
/// whole, with its length, as ever.
#[test]
fn awaits_stream_after_the_page() {
    let s = start();
    let r = s.request("GET", "/await", "", b"");
    assert_eq!(status(&r), 200, "{r}");
    assert_eq!(header(&r, "transfer-encoding"), Some("chunked"), "{r}");
    assert_eq!(header(&r, "content-length"), None);
    assert!(header(&r, "content-security-policy").is_some_and(|p| p.contains("'sha256-")));
    // The page is a chunk of its own, the awaits' pending markup in place.
    let (page, tail) = r.split_once("</html>").unwrap_or_else(|| panic!("{r}"));
    assert!(
        tail.trim_start().starts_with(char::is_alphanumeric),
        "a chunk's size: {tail}"
    );
    assert!(
        page.contains("<h1>Awaits</h1>")
            && page
                .contains("<wisp-await id=\"wisp-await-0\"><p id=\"a\">Loading a</p></wisp-await>")
            && page.contains(
                "<div id=\"c\"><wisp-await id=\"wisp-await-2\"><p>Loading c</p></wisp-await></div>"
            )
            && page.contains("<p id=\"end\">End</p>")
            && !page.contains("data-wisp-await"),
        "{page}"
    );
    let at = |k: u32| {
        tail.find(&format!("<div data-wisp-await=\"{k}\">"))
            .expect("each answer")
    };
    assert!(
        tail.contains("<div data-wisp-await=\"0\"><p id=\"a\">Got 7</p></div><script>")
            && tail.contains("<p id=\"b\">Failed: no luck</p>")
            && tail.contains(
                "<div data-wisp-await=\"2\"><p role=\"alert\">Something went wrong</p></div>"
            )
            && tail.contains("<div data-wisp-await=\"3\"><p id=\"d\">Quick</p></div>")
            && tail.ends_with("0\r\n\r\n"),
        "{tail}"
    );
    // As they come: the slow one last.
    assert!(at(3) < at(0) && at(1) < at(0) && at(2) < at(0), "{tail}");
    // The worker that ran the panic answers on.
    let plain = s.request("GET", "/await/plain", "", b"");
    assert!(plain.contains("<p id=\"w\">later</p></div>"), "{plain}");
    let whole = s.request("GET", "/signup", "", b"");
    assert!(
        header(&whole, "content-length").is_some() && header(&whole, "transfer-encoding").is_none()
    );
    // An answer's components: their instances numbered on from the page's,
    // in its div; a form field writes its own value.
    let e = &tail[at(4)..];
    let e = &e[..e.find("</div><script>").unwrap()];
    assert!(
        e.contains("<button class=\"tally\" data-w=\"1.0\">")
            && e.contains("<button class=\"step\" data-w=\"2.0\">")
            && e.contains("value=\"ann\"")
            && e.contains("<script type=\"application/json\" data-wisp-live>{\"m\":{")
            && e.contains("\"i\":[[1,\"t"),
        "{e}"
    );
    // gzip as a client asks, with `vary` either way.
    assert_eq!(header(&r, "vary"), Some("accept-encoding"));
    let gz = s.request("GET", "/await", "accept-encoding: gzip\r\n", b"");
    assert_eq!(header(&gz, "content-encoding"), Some("gzip"), "{gz}");
}

/// An answer waits no longer than `WISP_HANDLER_TIMEOUT`: then it is its
/// failure, and the response ends.
#[test]
fn an_await_gives_up_with_the_handler_timeout() {
    let s = start_with(&[("WISP_HANDLER_TIMEOUT", "1")]);
    let r = s.request("GET", "/await/slow/30000", "", b"");
    assert!(
        r.contains("<p role=\"alert\">Something went wrong</p>"),
        "{r}"
    );
    let quick = s.request("GET", "/await/slow/0", "", b"");
    assert!(quick.contains("<p id=\"s\">Done</p>"), "{quick}");
}
