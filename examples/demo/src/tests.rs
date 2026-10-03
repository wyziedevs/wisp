//! The demo in a real browser: `cargo test -p demo --features browser`.
//! Each test passes, skipped, where no Chrome or Edge is installed.
#![cfg(all(test, feature = "browser"))]

use crate::App;

#[test]
fn counter_counts() {
    let mut b = wisp::browser!(App);
    b.goto("/");
    assert_eq!(b.text("output"), "0");
    b.click("text=Plus One");
    b.click("text=Plus One");
    assert_eq!(b.text("output"), "2");
    b.click("text=Minus One");
    assert_eq!(b.text("output"), "1");
}

#[test]
fn casper_looks_at_the_pointer() {
    let mut b = wisp::browser!(App);
    b.goto("/");
    // The counter is below Casper, so his eyes look down.
    b.hover("text=Plus One");
    let style = b.attr(".casper", "style").unwrap_or_default();
    let y = style
        .split(';')
        .find_map(|d| d.trim().strip_prefix("--look-y:"))
        .and_then(|v| v.trim().parse::<f64>().ok());
    assert!(y.is_some_and(|y| y > 0.0), "{style}");
}

#[test]
fn wisple_takes_typed_guesses() {
    let mut b = wisp::browser!(App);
    b.goto("/wisple");
    for key in ["a", "b", "o", "u", "t"] {
        b.press(key);
    }
    let row = b.text(".row.current").to_lowercase();
    assert_eq!(row.split_whitespace().collect::<String>(), "about");
    b.press("Enter");
    assert_eq!(b.count(".letter.exact, .letter.close, .letter.missing"), 5);
    assert_eq!(b.text(".row.current"), "");
}

#[test]
fn links_inputs_and_screenshots() {
    let mut b = wisp::browser!(App);
    b.goto("/about");
    b.click("text=Play Wisple");
    assert!(b.url().ends_with("/wisple"), "{}", b.url());
    b.wait(".keyboard");
    b.eval("document.body.insertAdjacentHTML('beforeend', '<textarea id=note></textarea>')");
    b.fill("#note", "hello");
    assert_eq!(b.eval("document.querySelector('#note').value").as_str(), Some("hello"));
    let png = std::env::temp_dir().join(format!("wisp-demo-{}.png", std::process::id()));
    b.screenshot(&png);
    let bytes = std::fs::read(&png).unwrap();
    let _ = std::fs::remove_file(&png);
    assert!(bytes.starts_with(b"\x89PNG"));
}
