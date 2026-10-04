//! The site in each locale, in process: the prefix, the redirects, the
//! tags a crawler reads and the static export. `cargo test -p i18n`.

use super::App;
use wisp::test::client;

fn has(page: &str, want: &[&str]) {
    for w in want {
        assert!(page.contains(w), "{w}\n{page}");
    }
}

#[test]
fn english_has_no_prefix_and_the_rest_do() {
    let mut app = client::<App>();
    // The default locale, whatever the visitor's language is.
    app.header("accept-language", "fr");
    let en = app.get("/about");
    assert_eq!(en.status, 200);
    has(
        en.text(),
        &[
            "<html lang=\"en\">",
            "<h1>About this site</h1>",
            "Written for you, in English.",
            "<a href=\"/about\" lang=\"en\" hreflang=\"en\" aria-current=\"true\">English</a>",
            "<a href=\"/fr/about\" lang=\"fr\" hreflang=\"fr\">Français</a>",
        ],
    );
    let fr = app.get("/fr/about");
    has(
        fr.text(),
        &["<html lang=\"fr\">", "Écrit pour you, en français."],
    );
    // `/en/about` is `/about`, for good, with its query.
    let moved = app.get("/en/about?x=1");
    assert_eq!(moved.status, 308);
    assert_eq!(moved.location(), Some("/about?x=1"));
    assert_eq!(app.get("/de/about").status, 404);
}

#[test]
fn arabic_reads_right_to_left() {
    let mut app = client::<App>();
    let ar = app.get("/ar/about");
    has(
        ar.text(),
        &[
            "<html lang=\"ar\" dir=\"rtl\">",
            "<h1>حول هذا الموقع</h1>",
            "كُتب من أجل you، بالعربية.",
            "<header dir=\"rtl\">",
        ],
    );
    // Arabic's plural has its own cases.
    has(app.get("/ar?count=2").text(), &["عنصران في سلتك"]);
}

#[test]
fn numbers_money_and_dates_follow_the_locale() {
    let mut app = client::<App>();
    has(
        app.get("/").text(),
        &[
            "<p class=\"number\">1,234,567.891</p>",
            "<p class=\"money\">€1,234.50</p>",
            "<p class=\"date\">October 4, 2026</p>",
            "Your cart is empty",
        ],
    );
    has(
        app.get("/fr?count=1").text(),
        &[
            "<p class=\"number\">1\u{202f}234\u{202f}567,891</p>",
            "<p class=\"money\">1\u{202f}234,50\u{a0}€</p>",
            "<p class=\"date\">4 octobre 2026</p>",
            "1 article dans votre panier",
        ],
    );
}

#[test]
fn crawlers_get_canonical_and_alternate_tags() {
    let mut app = client::<App>();
    app.header("host", "example.com");
    let page = app.get("/fr/about");
    has(
        page.text(),
        &[
            "<link rel=\"canonical\" href=\"https://example.com/fr/about\">",
            "<link rel=\"alternate\" hreflang=\"ar\" href=\"https://example.com/ar/about\">",
            "<link rel=\"alternate\" hreflang=\"en\" href=\"https://example.com/about\">",
            "<link rel=\"alternate\" hreflang=\"fr\" href=\"https://example.com/fr/about\">",
            "<link rel=\"alternate\" hreflang=\"x-default\" href=\"https://example.com/about\">",
        ],
    );
    app.header("host", "example.com");
    let map = app.get("/sitemap.xml");
    has(
        map.text(),
        &[
            "xmlns:xhtml=\"http://www.w3.org/1999/xhtml\"",
            "<url><loc>https://example.com/</loc>",
            "<url><loc>https://example.com/fr</loc>",
            "<url><loc>https://example.com/ar/about</loc>",
            "<xhtml:link rel=\"alternate\" hreflang=\"fr\" href=\"https://example.com/fr/about\"/>",
            "<xhtml:link rel=\"alternate\" hreflang=\"x-default\" href=\"https://example.com/about\"/>",
        ],
    );
    assert!(!map.text().contains("/en/"), "{}", map.text());
}

#[test]
fn the_static_export_writes_each_locale() {
    let dir = std::env::temp_dir().join(format!("wisp-i18n-{}", std::process::id()));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(wisp::export::<App>(&dir, false)).unwrap();
    let read =
        |p: &str| std::fs::read_to_string(dir.join(p)).unwrap_or_else(|e| panic!("{p}: {e}"));
    has(&read("index.html"), &["<html lang=\"en\">", "Hello, world"]);
    has(
        &read("fr/index.html"),
        &["<html lang=\"fr\">", "Bonjour le monde"],
    );
    has(
        &read("fr/about/index.html"),
        &["Écrit pour you, en français."],
    );
    has(
        &read("ar/about/index.html"),
        &["<html lang=\"ar\" dir=\"rtl\">"],
    );
    // English has no prefix.
    assert!(!dir.join("en").exists());
    let _ = std::fs::remove_dir_all(&dir);
}
