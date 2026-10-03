//! In a headless Chrome or Edge (`cargo test -p wisp-test-platform
//! --features browser`; skipped where there is none).
#![cfg(feature = "browser")]

use wisp_test_platform::Site;

/// `<x-card>` on a page that is not Wisp's, with one script tag: drawn in
/// its shadow root with its CSS, its attributes read as their Rust types,
/// its properties, its children in its `<slot>`, its events.
#[test]
fn a_custom_element_on_any_page() {
    let mut b = wisp::browser!(Site);
    b.goto("/embed.html");
    let card = "document.querySelector('x-card')";
    let root = format!("{card}.shadowRoot");
    let ready = format!(
        "customElements.whenDefined('x-card').then(() => new Promise((r) => setTimeout(r, 50))).then(() => {root}.querySelector('h2').textContent)"
    );
    assert_eq!(b.eval(&ready).as_str(), Some("Hi ★"));
    let text = |b: &mut wisp::test::Browser, sel: &str| {
        let js = format!(
            "new Promise((r) => setTimeout(r, 20)).then(() => {root}.querySelector({sel:?}).textContent)"
        );
        b.eval(&js).as_str().unwrap_or_default().to_string()
    };
    assert_eq!(text(&mut b, "button"), "5", "count=\"5\" is a number");
    assert_eq!(text(&mut b, ".tags"), "a, b", "tags is JSON");
    assert_eq!(text(&mut b, ".badge"), "Hi", "the component it draws");
    b.eval(&format!("{root}.querySelector('button').click()"));
    assert_eq!(
        text(&mut b, "button"),
        "6",
        "on:click inside the shadow root"
    );
    b.eval(&format!("{card}.setAttribute('title', 'New')"));
    assert_eq!(text(&mut b, "h2"), "New ★");
    b.eval(&format!("{card}.featured = false; {card}.count = 9"));
    assert_eq!(text(&mut b, "h2"), "New", "a property");
    assert_eq!(text(&mut b, "button"), "9");
    b.eval(&format!("{card}.removeAttribute('title')"));
    assert_eq!(text(&mut b, "h2"), "");
    assert_eq!(
        b.eval(&format!(
            "{root}.querySelector('slot').assignedNodes()[0].textContent"
        ))
        .as_str(),
        Some("Inside")
    );
    let color = |b: &mut wisp::test::Browser, sel: &str| {
        let js = format!("getComputedStyle({root}.querySelector({sel:?})).color");
        b.eval(&js).as_str().unwrap_or_default().to_string()
    };
    assert_eq!(color(&mut b, "h2"), "rgb(0, 128, 0)", "its scoped style");
    assert_eq!(color(&mut b, ".badge"), "rgb(0, 0, 255)", "and its child's");
    // Moved, it keeps its state.
    b.eval(&format!("document.body.prepend({card})"));
    assert_eq!(text(&mut b, "button"), "9");
}
