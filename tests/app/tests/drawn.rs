//! A page with `const SSR: bool = false;`, drawn by a headless Chrome or
//! Edge from the data the server sent (`cargo test -p wisp-test-app
//! --features browser`; skipped where there is none).
#![cfg(feature = "browser")]

use wisp_test_app::Site;

#[test]
fn the_browser_draws_the_page() {
    let mut b = wisp::browser!(Site);
    b.goto("/drawn");
    assert_eq!(b.text("h1"), "Tea");
    assert_eq!(b.count("li"), 3);
    b.click("text=Clicked 0");
    assert_eq!(b.text("button"), "Clicked 1");
}
