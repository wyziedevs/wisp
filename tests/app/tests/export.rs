//! `wisp build --static`'s export of the test app, into a temp folder.

mod common;

use common::Temp;
use wisp_test_app::Site;

#[test]
fn writes_pages_and_assets() {
    let dir = Temp::new("export");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(wisp::export::<Site>(&dir)).unwrap();

    let read =
        |p: &str| std::fs::read_to_string(dir.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    assert!(read("index.html").contains("<h1>hello from init</h1>"));
    assert!(read("login/index.html").contains("<form"));
    assert!(read("post/hello/index.html").contains("<h1>Post hello</h1>"));
    assert!(read("post/second-post/index.html").contains("Post second-post"));
    assert!(read("404.html").contains("There is nothing at this address."));
    assert!(read("_app/wisp.js").len() > 100);
    // A redirect, and an endpoint, are not pages.
    assert!(!dir.join("admin").exists() && !dir.join("echo").exists());
}
