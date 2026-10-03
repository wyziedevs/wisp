//! React islands in a headless Chrome or Edge: `cargo test -p
//! wisp-islands --features browser`. React loads from esm.sh, as in dev.

use crate::App;

#[test]
fn the_page_paints_without_react() {
    let mut app = wisp::test::client::<App>();
    let page = app.get("/").text().to_string();
    assert!(page.contains("data-wisp-keep"), "{page}");
    assert!(page.contains(">Loading</div>"), "{page}");
}

#[cfg(feature = "browser")]
#[test]
fn react_components_mount_and_talk_back() {
    let mut b = wisp::browser!(App);
    b.timeout(std::time::Duration::from_secs(30));
    b.goto("/");
    // A hook works: one copy of React for the page, react-dom and the file.
    b.click("text=Count: 2");
    b.click("text=Count: 3");
    assert_eq!(b.text("text=Count: 4"), "Count: 4");
    // A package from npm, its callback setting the page's state.
    assert_eq!(b.text("output"), "off");
    b.click(".react-switch-handle");
    assert_eq!(b.text("output"), "on");
    assert_eq!(
        b.attr("[role=switch]", "aria-checked").as_deref(),
        Some("true")
    );
}
