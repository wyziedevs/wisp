//! Code loaded on demand (`src/routes/lazy`): `import()` resolves as a
//! static import does, a page preloads what it imports statically and not
//! what it imports later, and code two pages import is one module.

#![cfg(not(target_arch = "wasm32"))]

use wisp::test::{Client, client};
use wisp_test_app::Site;

/// The page's `modulepreload` URLs, and its own module's source.
fn page(app: &mut Client<Site>, path: &str) -> (Vec<String>, String) {
    let html = app.get(path).text().to_string();
    let mut preloads = Vec::new();
    for part in html.split("<link rel=\"modulepreload\" href=\"").skip(1) {
        preloads.push(part[..part.find('"').unwrap()].to_string());
    }
    let own = preloads
        .iter()
        .find(|p| p.starts_with("/_app/c/t"))
        .unwrap();
    let js = app.get(own).text().to_string();
    (preloads, js)
}

#[test]
fn dynamic_imports_resolve_and_wait() {
    let mut app = client::<Site>();
    let (preloads, js) = page(&mut app, "/lazy");
    // `$lib/x.js` and a relative path, in a directive and in the script.
    assert!(js.contains("import(\"/_app/c/lib/names.js?v="), "{js}");
    assert!(
        js.contains("await import(\"/_app/c/lib/calls.js?v="),
        "{js}"
    );
    // What it imports statically is preloaded, all the way down (calls.js
    // imports the #[remote] functions); `import()` is not.
    let has = |p: &str| preloads.iter().any(|u| u.starts_with(p));
    assert!(
        has("/_app/c/lib/calls.js?v=") && has("/_app/c/remote.js?v="),
        "{preloads:?}"
    );
    assert!(!has("/_app/c/lib/names.js"), "{preloads:?}");
    assert!(!has("/_app/live.js"), "the page loads it itself");
}

#[test]
fn code_two_pages_import_is_one_module() {
    let mut app = client::<Site>();
    let (lazy, _) = page(&mut app, "/lazy");
    let (remote, _) = page(&mut app, "/remote");
    let calls = |p: &[String]| p.iter().find(|u| u.contains("/lib/calls.js")).cloned();
    assert!(calls(&lazy).is_some());
    assert_eq!(calls(&lazy), calls(&remote));
    let url = calls(&lazy).unwrap();
    let r = app.get(&url);
    assert_eq!(r.status, 200);
    assert!(
        r.header("cache-control").unwrap().contains("immutable"),
        "fetched once"
    );
}

#[cfg(feature = "browser")]
#[test]
fn a_browser_loads_on_demand() {
    let mut b = wisp::browser!(Site);
    b.goto("/lazy");
    b.click("text=Load");
    b.wait("text=LAZY");
    b.click("text=Sum");
    b.wait("text=5");
    assert_eq!(b.text("output"), "5");
}
