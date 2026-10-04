//! `redirects`, `rewrites` and `headers` of `[package.metadata.wisp]` in
//! this app's Cargo.toml.

#![cfg(not(target_arch = "wasm32"))]

use wisp::test::client;
use wisp_test_app::Site;

#[test]
fn redirects_rewrites_and_headers() {
    let mut app = client::<Site>();
    let r = app.get("/old/hello?a=1");
    assert_eq!((r.status, r.location()), (301, Some("/post/hello?a=1")));
    let r = app.get("/off");
    assert_eq!(
        (r.status, r.location()),
        (308, Some("https://example.com/x"))
    );
    // A rewrite is served by its route, the address kept.
    let r = app.get("/p/second-post");
    assert_eq!(r.status, 200);
    assert!(r.text().contains("Post second-post"));
    assert_eq!(app.get("/p/").status, 308);
    // A header of the rules, on the replies of the paths it names only.
    assert_eq!(app.get("/post/hello").header("x-rule"), Some("yes"));
    assert_eq!(app.get("/login").header("x-rule"), None);
    assert_eq!(app.get("/old/x/y").status, 404);
}
