//! The test client's helpers: `location`, `upload`, `sign_in`, `modules`
//! and `websocket`.

#![cfg(not(target_arch = "wasm32"))]

use wisp::test::client;
use wisp_test_app::Site;

#[test]
fn location_names_a_redirect() {
    let mut app = client::<Site>();
    let r = app.get("/me");
    assert_eq!(r.status, 303);
    assert_eq!(r.location(), Some("/login"));
    assert_eq!(app.get("/").location(), None);
}

#[test]
fn upload_sends_a_file() {
    let mut app = client::<Site>();
    let r = app.upload("/upload", "photo", "image/png", &[7u8; 300]);
    let page = r.text();
    assert!(page.contains("upload: 300 bytes of image/png"), "{page}");
    let r = app.upload("/upload", "other", "image/png", &[7u8; 3]);
    assert!(r.text().contains("Choose a photo"));
}

#[test]
fn sign_in_needs_no_login_form() {
    let mut app = client::<Site>();
    // Someone else joins; this client signs in as them without a password.
    let r = client::<Site>().post_form(
        "/join?/join",
        &[("name", "Ada"), ("password", "correct horse")],
    );
    assert_eq!(r.location(), Some("/me"));
    app.sign_in(1);
    let page = app.get("/me");
    assert_eq!(page.status, 200);
    assert!(page.text().contains("<h1>Ada</h1>"), "{}", page.text());
}

#[test]
fn modules_are_fetched_with_their_source() {
    let mut app = client::<Site>();
    let page = app.get("/live");
    let modules = app.modules(&page);
    assert!(!modules.is_empty(), "{}", page.text());
    for (url, source) in &modules {
        assert!(url.starts_with("/_app/c/") && !source.is_empty(), "{url}");
    }
}

#[test]
fn a_websocket_echoes() {
    let mut app = client::<Site>();
    let mut ws = app.websocket("/ws");
    ws.send("hello");
    assert_eq!(ws.recv(), Some("hello".into()));
    ws.send(vec![1u8, 2, 3]);
    assert_eq!(ws.recv(), Some(vec![1u8, 2, 3].into()));
    ws.close();
    assert_eq!(ws.recv(), None);
}

#[test]
#[should_panic(expected = "expected an upgrade")]
fn a_route_that_does_not_upgrade_panics() {
    client::<Site>().websocket("/");
}
