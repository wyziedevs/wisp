//! Client features in a headless Chrome or Edge (`cargo test -p
//! wisp-test-app --features browser`; skipped where there is none).
#![cfg(feature = "browser")]

use wisp_test_app::Site;

/// pushState and replaceState: `page.value.state` follows the history,
/// and back and forward to an entry made so ask the server nothing.
#[test]
fn shallow_routing() {
    let mut b = wisp::browser!(Site);
    b.goto("/a2/shallow");
    let n = b.text("#n");
    assert_eq!(b.text("output"), "1");
    b.click("#two");
    assert_eq!(b.text("output"), "2");
    assert!(b.url().ends_with("/a2/shallow?tab=2"), "{}", b.url());
    b.eval("history.back()");
    assert_eq!(b.text("output"), "1");
    assert!(b.url().ends_with("/a2/shallow"));
    b.eval("history.forward()");
    assert_eq!(b.text("output"), "2");
    b.click("#three");
    assert_eq!(b.text("output"), "3");
    assert!(b.url().ends_with("?tab=2"), "replaceState('') keeps the URL");
    b.eval("history.back()");
    assert_eq!(b.text("output"), "1");
    b.eval("history.forward()");
    assert_eq!(b.text("output"), "3");
    assert_eq!(b.text("#n"), n, "no request");

    // Away and back: that entry's page from the server, with its state.
    b.click("#away");
    assert_eq!(b.text("h1"), "Islands");
    b.eval("history.back()");
    assert_eq!(b.text("output"), "3");
    assert_ne!(b.text("#n"), n);
}

/// An island in a server component (no script: no JS) in an island, as
/// the outer one's children and in its own markup.
#[test]
fn islands_inside_server_components_inside_islands() {
    let mut b = wisp::browser!(Site);
    b.goto("/a2/server");
    for at in ["#slotted", "#direct"] {
        b.wait(&format!("{at} .ping[data-on]"));
        b.click(&format!("{at} .outer"));
        assert_eq!(b.text(&format!("{at} .outer")), "Outer 1");
        b.click(&format!("{at} .tally"));
        assert_eq!(b.text(&format!("{at} .tally")), "1");
    }
    // The inner island wakes at its own moment, and wakes the outer one,
    // which waits for a touch, with it.
    b.wait("#lazy .ping[data-on]");
    b.click("#lazy .outer");
    assert_eq!(b.text("#lazy .outer"), "Outer 1");
    b.click("#lazy .tally");
    assert_eq!(b.text("#lazy .tally"), "1");
}
