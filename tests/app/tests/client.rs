//! The test app answering in process, through `wisp::test::client`: the
//! same parser, limits, hooks and pages as on the wire, with no server.

use wisp::test::client;
use wisp::{Body, Request};
use wisp_test_app::Site;

#[test]
fn pages_hooks_and_errors() {
    let mut app = client::<Site>();
    let home = app.get("/");
    assert_eq!(home.status, 200);
    assert!(
        home.text().contains("<h1>hello from init</h1>"),
        "{}",
        home.text()
    );
    assert!(
        home.text().contains("/_app/wisp.js?v="),
        "the head tags are in: {}",
        home.text()
    );
    assert_eq!(
        home.header("content-type"),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(home.header("x-app"), Some("test"));

    let missing = app.get("/nope");
    assert_eq!(
        (missing.status, missing.header("x-app")),
        (404, Some("test"))
    );
    assert!(missing.text().contains("There is nothing at this address."));

    let slash = app.get("/login/?a=1");
    assert_eq!(
        (slash.status, slash.header("location")),
        (308, Some("/login?a=1"))
    );
}

#[test]
fn sign_in_keeps_cookies() {
    let mut app = client::<Site>();
    assert_eq!(app.get("/admin").header("location"), Some("/login"));
    let login = app.post_form("/login", &[("name", "ada")]);
    assert_eq!(
        (login.status, login.header("location")),
        (303, Some("/admin"))
    );
    assert!(app.cookie("user").is_some_and(|c| c.starts_with("ada.")));
    let admin = app.get("/admin");
    assert_eq!(admin.status, 200);
    assert!(admin.text().contains("Welcome, ada"));

    let bad = app.post_form("/login", &[("name", "a d&a")]);
    assert_eq!(bad.status, 400);
}

#[test]
fn same_limits_as_the_server() {
    let mut app = client::<Site>();
    let mut big = Request::new("POST", "/echo");
    big.body = vec![b'a'; 100 * 1024];
    assert_eq!(app.send(big).status, 413);

    let mut echo = Request::new("POST", "/echo");
    echo.body = b"hi".to_vec();
    // Framing is the host's: the body given is the body.
    echo.header("transfer-encoding", "chunked");
    echo.header("content-length", "99");
    assert_eq!(app.send(echo).text(), "2:hi");

    let mut split = Request::new("GET", "/");
    split.header("x-a", "1\r\nx-b: 2");
    assert_eq!(app.send(split).status, 400);
    assert_eq!(
        app.send(Request::new("GET", "/ HTTP/1.1\r\nx: y")).status,
        400
    );
    let mut huge = Request::new("GET", "/");
    huge.header("x-big", &"a".repeat(20 * 1024));
    assert_eq!(app.send(huge).status, 431);
}

#[test]
fn streams_and_files() {
    let mut app = client::<Site>();
    let mut events = app.get("/events");
    assert_eq!(events.header("content-type"), Some("text/event-stream"));
    assert!(matches!(events.body, Body::Stream(_)));
    for i in 0..3 {
        let chunk = app.next_chunk(&mut events).unwrap();
        assert_eq!(
            String::from_utf8(chunk).unwrap(),
            format!("data: tick {i}\ndata: line two\n\n")
        );
    }
    assert_eq!(app.next_chunk(&mut events), None);

    let js = app.get("/_app/wisp.js");
    assert_eq!(js.status, 200);
    let etag = js.header("etag").unwrap().to_string();
    let mut again = Request::new("GET", "/_app/wisp.js");
    again.header("if-none-match", &etag);
    let again = app.send(again);
    assert_eq!(
        (
            again.status,
            again.bytes().len(),
            again.header("content-type")
        ),
        (304, 0, None)
    );
}

#[test]
fn head_has_the_headers_and_no_body() {
    let mut app = client::<Site>();
    let get = app.get("/");
    let head = app.send(Request::new("HEAD", "/"));
    assert_eq!(head.status, 200);
    assert_eq!(head.header("content-type"), get.header("content-type"));
    assert!(head.bytes().is_empty());
    assert_eq!(
        head.header("content-length").map(str::to_string),
        Some(get.bytes().len().to_string())
    );
}

#[test]
fn no_content_has_no_body_or_length() {
    let mut app = client::<Site>();
    let preflight = app.send(Request::new("OPTIONS", "/echo"));
    assert_eq!(
        (
            preflight.status,
            preflight.bytes().len(),
            preflight.header("content-length")
        ),
        (204, 0, None)
    );
}
