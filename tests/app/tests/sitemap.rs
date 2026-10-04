//! `/sitemap.xml` and `/robots.txt`, made from the routes.

#![cfg(not(target_arch = "wasm32"))]

use wisp::test::client;
use wisp_test_app::Site;

#[test]
fn sitemap_and_robots() {
    let mut app = client::<Site>();
    app.header("host", "example.com");
    let map = app.get("/sitemap.xml");
    assert_eq!(map.status, 200);
    assert_eq!(
        map.header("content-type"),
        Some("application/xml; charset=utf-8")
    );
    let text = map.text().to_string();
    for want in [
        "<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\" xmlns:xhtml=\"http://www.w3.org/1999/xhtml\">",
        // A page under `[[lang=locale]]`, in each locale, with its alternates.
        "<url><loc>https://example.com/en/i18n</loc><xhtml:link rel=\"alternate\" hreflang=\"en\" href=\"https://example.com/en/i18n\"/><xhtml:link rel=\"alternate\" hreflang=\"fr\" href=\"https://example.com/fr/i18n\"/><xhtml:link rel=\"alternate\" hreflang=\"x-default\" href=\"https://example.com/en/i18n\"/></url>",
        "<url><loc>https://example.com/fr/i18n</loc>",
        "<url><loc>https://example.com/</loc></url>",
        "<url><loc>https://example.com/blog/hello</loc></url>",
        // `entries()` of a `[slug]` page.
        "<url><loc>https://example.com/post/second-post</loc></url>",
    ] {
        assert!(text.contains(want), "{want}\n{text}");
    }
    // Not a param without entries, a `(private)` group, a noindex page.
    for not in ["[", "/hidden", "/blog/draft", "/users/"] {
        assert!(!text.contains(not), "{not}\n{text}");
    }

    // No host and no `SITE_URL`: no address to give.
    assert_eq!(app.get("/sitemap.xml").status, 404);
    // Only GET and HEAD.
    app.header("host", "example.com");
    assert_eq!(app.post_form("/sitemap.xml", &[]).status, 404);

    // `static/robots.txt` is served instead of the one made.
    let robots = app.get("/robots.txt");
    assert_eq!(robots.status, 200);
    assert_eq!(
        robots.text().replace("\r\n", "\n"),
        "User-agent: *\nDisallow: /admin\n"
    );
}
