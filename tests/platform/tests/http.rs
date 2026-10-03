//! The app's answers, in process.

use wisp_test_platform::Site;

/// A custom element's module, and what it imports, load from any site.
#[test]
fn element_modules_allow_any_origin() {
    let mut app = wisp::test::client::<Site>();
    let r = app.get("/_app/c/el/x-card.js");
    assert_eq!(r.status, 200);
    assert_eq!(r.header("access-control-allow-origin"), Some("*"));
    assert!(r.text().contains("element(\"x-card\", "), "{}", r.text());
    let runtime = r.text().split('"').nth(1).unwrap().to_string();
    assert!(runtime.starts_with("/_app/c/el.js?v="), "{runtime}");
    let r = app.get(&runtime);
    assert_eq!(r.header("access-control-allow-origin"), Some("*"));
    assert!(r.text().contains("customElements.define"));
    let at = r.text().find("/_app/live.js?v=").unwrap();
    let live = r.text()[at..].split('"').next().unwrap().to_string();
    let r = app.get(&live);
    assert_eq!(r.status, 200, "{live}");
    assert_eq!(r.header("access-control-allow-origin"), Some("*"));
    // The page that renders it on the server still does.
    assert!(app.get("/").text().contains("On a page"));
}
