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

/// `wisp build --static` writes the worker, the manifest, and the
/// element's module with all it imports, which no page names.
#[test]
fn a_static_export_has_them() {
    let dir = std::env::temp_dir().join(format!("wisp-platform-export-{}", std::process::id()));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(wisp::export::<Site>(&dir, false)).unwrap();
    let has = |p: &str| dir.join(p).is_file();
    let found = [
        "service-worker.js",
        "manifest.webmanifest",
        "_app/c/el/x-card.js",
        "_app/c/el.js",
        "_app/live.js",
    ]
    .map(has);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(found, [true; 5]);
}

/// Every page links the manifest and registers the worker, which its
/// content-security-policy allows.
#[test]
fn manifest_and_service_worker() {
    let mut app = wisp::test::client::<Site>();
    let page = app.get("/");
    let html = page.text();
    assert!(
        html.contains("<link rel=\"manifest\" href=\"/manifest.webmanifest\">"),
        "{html}"
    );
    let register = "navigator.serviceWorker?.register(\"/service-worker.js\")";
    assert!(html.contains(&format!("<script>{register}</script>")));
    let csp = page.header("content-security-policy").unwrap();
    assert!(csp.contains("worker-src 'self'"), "{csp}");
    assert!(csp.contains("script-src 'self' 'sha256-"), "{csp}");

    let m = app.get("/manifest.webmanifest");
    assert_eq!(m.status, 200);
    assert_eq!(m.header("content-type"), Some("application/manifest+json"));
    assert_eq!(
        m.text(),
        "{\"name\":\"Platform\",\"theme_color\":\"#7c3aed\",\"short_name\":\"Platform\",\"start_url\":\"/\",\"display\":\"standalone\",\
         \"icons\":[{\"src\":\"/icon-192.png\",\"sizes\":\"192x192\",\"type\":\"image/png\"}]}"
    );

    let w = app.get("/service-worker.js");
    assert_eq!(w.status, 200);
    assert!(
        w.header("content-type")
            .unwrap()
            .starts_with("text/javascript")
    );
    assert_eq!(
        w.header("cache-control"),
        Some("public, max-age=0, must-revalidate")
    );
    let w = w.text();
    assert!(
        w.starts_with("// Wisp's offline service worker.\nconst build = ["),
        "{w}"
    );
    if cfg!(debug_assertions) {
        // A debug build keeps nothing but the shell.
        assert!(
            w.contains("const build = [], files = [], version = \""),
            "{w}"
        );
    } else {
        // A release build keeps its browser files and `static/`.
        assert!(w.contains("\"/_app/wisp.js?v="), "{w}");
        assert!(w.contains("\"/_app/c/el.js?v="), "{w}");
        assert!(!w.contains("/_app/c/el/x-card.js"), "for other sites: {w}");
        assert!(
            w.contains("files = [\"/embed.html\", \"/icon-192.png\"]"),
            "{w}"
        );
    }
}
