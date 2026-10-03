//! Client features in a headless Chrome or Edge (`cargo test -p
//! wisp-test-app --features browser`; skipped where there is none).
#![cfg(feature = "browser")]

use wisp_test_app::Site;

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
