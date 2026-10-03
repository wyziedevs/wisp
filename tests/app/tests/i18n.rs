//! Translations: `src/locales/en.json` and `fr.json`, the page
//! `[[lang=locale]]/i18n`, and the locale each request gets.

use wisp::test::client;
use wisp_test_app::Site;

fn has(page: &str, want: &[&str]) {
    for w in want {
        assert!(page.contains(w), "{w}\n{page}");
    }
}

#[test]
fn the_url_names_the_locale() {
    let mut app = client::<Site>();
    let en = app.get("/i18n?n=1");
    assert_eq!(en.status, 200);
    has(
        en.text(),
        &[
            "<html lang=\"en\">",
            "<title>Your cart</title>",
            "<p class=\"count\">1 item</p>",
            "<p class=\"hi\">Hello, &lt;Ann&gt;!</p>",
            "<p class=\"locale\">en</p>",
            "<span class=\"badge\">2 items</span>",
            "<a href=\"/en/i18n\">en</a>",
            "<a href=\"/fr/i18n\">fr</a>",
        ],
    );
    let fr = app.get("/fr/i18n?n=0");
    let fr = fr.text();
    has(
        fr,
        &[
            "<html lang=\"fr\">",
            "<h1>Votre panier</h1>",
            "<p class=\"count\">Aucun article</p>",
            "<p class=\"hi\">Bonjour, &lt;Ann&gt; !</p>",
            "<p class=\"locale\">fr</p>",
            "<span class=\"badge\">1 article</span>",
            "<b class=\"hello\">Bonjour, Bo !</b>",
        ],
    );
    // Only the messages its script shows go to the browser, in its locale.
    has(
        fr,
        &[r#""t":{"i18n.added":[["count",{"one":["Un ajouté"],"other":[["count"]," ajoutés"]}]]}"#],
    );
    assert!(!fr.contains("Aucun article\"") && !fr.contains("\"i18n.title\""));
    // Not a locale: no route.
    assert_eq!(app.get("/de/i18n").status, 404);
}

#[test]
fn a_cookie_then_accept_language_pick_it() {
    let mut app = client::<Site>();
    app.header("accept-language", "de, fr-CA;q=0.8, en;q=0.5");
    has(
        app.get("/i18n?n=2").text(),
        &["<p class=\"count\">2 articles</p>"],
    );
    app.header("cookie", "lang=en");
    app.header("accept-language", "fr");
    has(
        app.get("/i18n?n=2").text(),
        &["<p class=\"count\">2 items</p>"],
    );
    // The URL wins over both.
    app.header("cookie", "lang=en");
    has(app.get("/fr/i18n").text(), &["<html lang=\"fr\">"]);
    assert_eq!(wisp::locales(), ["en", "fr"]);
    assert_eq!(wisp::localize("/fr/i18n?n=1", "en"), "/en/i18n?n=1");
}
