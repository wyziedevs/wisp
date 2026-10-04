//! `src/routes/t/forms`: an action that takes a struct, inputs that show
//! their own value or what was sent, and every problem at once.

#![cfg(not(target_arch = "wasm32"))]

use wisp::test::client;
use wisp_test_app::Site;

#[test]
fn an_edit_form() {
    let mut app = client::<Site>();
    let page = app.get("/t/forms").text().to_string();
    for want in [
        // The browser checks first what it can of the rules.
        "<input aria-label=\"title\" name=\"title\" value=\"First\" required minlength=\"1\" pattern=\"[\\s\\S]{0,20}\">",
        "<textarea aria-label=\"body\" name=\"body\" required minlength=\"3\">Hello</textarea>",
        "<option value=\"a\">A</option><option value=\"b\" selected>B</option>",
        "<input aria-label=\"stars\" name=\"stars\" value=\"3\" required>",
        "<input aria-label=\"note\" name=\"note\">",
    ] {
        assert!(page.contains(want), "{want} in {page}");
    }
    assert!(!page.contains("<small"), "{page}");

    // Every field that does not pass, each with what was sent and its
    // problem; a blank required field is missing.
    let fields = [
        ("title", " "),
        ("body", "hi"),
        ("kind", "a"),
        ("stars", "x"),
    ];
    let bad = app.post_form("/t/forms", &fields);
    assert_eq!(bad.status, 422);
    let html = bad.text();
    for want in [
        "<input aria-label=\"title\" name=\"title\" value=\" \" required minlength=\"1\" pattern=\"[\\s\\S]{0,20}\"><small class=\"problem\">is required</small>",
        "<textarea aria-label=\"body\" name=\"body\" required minlength=\"3\">hi</textarea><small class=\"problem\">must have at least 3 characters</small>",
        "<option value=\"a\" selected>A</option><option value=\"b\">B</option></select>",
        "<input aria-label=\"stars\" name=\"stars\" value=\"x\" required><small class=\"problem\">expected a whole number</small>",
    ] {
        assert!(html.contains(want), "{want} in {html}");
    }

    // Right, it is saved, and the form shows it.
    let fields = [
        ("title", "Second"),
        ("body", "Longer"),
        ("kind", "a"),
        ("stars", "5"),
        ("draft", "on"),
        ("note", ""),
    ];
    assert_eq!(app.post_form("/t/forms", &fields).status, 303);
    let page = app.get("/t/forms").text().to_string();
    for want in [
        "<input aria-label=\"title\" name=\"title\" value=\"Second\" required",
        "<option value=\"a\" selected>A</option>",
        "<input aria-label=\"draft\" name=\"draft\" type=\"checkbox\" checked>",
    ] {
        assert!(page.contains(want), "{want} in {page}");
    }

    // A JSON body is read whole: its problems by field too.
    let json = app.post_json(
        "/t/forms",
        r#"{"title":"","body":"x","kind":"a","stars":1}"#,
    );
    assert_eq!(json.status, 422);
    assert!(
        json.text().contains(r#""errors":{"title":"must have at least 1 character","body":"must have at least 3 characters"}"#),
        "{}",
        json.text()
    );
}

#[test]
fn every_problem_at_once() {
    let mut app = client::<Site>();
    let bad = app.post_form(
        "/t/forms?/pair",
        &[("a", "x"), ("n", "1"), ("pw", "secret")],
    );
    assert_eq!(bad.status, 422);
    let html = bad.text();
    for want in [
        "<input aria-label=\"a\" name=\"a\" required minlength=\"2\" value=\"x\"><small class=\"problem\">must have at least 2 characters</small>",
        // Shown where the page puts it, and so not after its input.
        "<input aria-label=\"n\" name=\"n\" type=\"number\" required min=\"5\" value=\"1\"><input",
        "<p id=\"n-problem\"><small class=\"problem\">must be at least 5</small></p>",
        // A password is never sent back.
        "<input aria-label=\"pw\" name=\"pw\" type=\"password\">\n<p",
    ] {
        assert!(html.contains(want), "{want} in {html}");
    }
    assert!(!html.contains("secret"), "{html}");

    // Not a number is a problem of its field, listed with the others.
    let bad = app.post_form("/t/forms?/pair", &[("a", "x"), ("n", "many"), ("pw", "")]);
    assert_eq!(bad.status, 422);
    assert!(
        bad.text()
            .contains("<small class=\"problem\">invalid digit found in string</small>"),
        "{}",
        bad.text()
    );
    assert!(
        bad.text().contains("must have at least 2 characters"),
        "{}",
        bad.text()
    );

    // From JSON, each missing field is listed.
    let json = app.post_json("/t/forms?/pair", "{}");
    assert_eq!(json.status, 422);
    assert!(
        json.text()
            .contains(r#""errors":{"a":"is required","n":"is required","pw":"is required"}"#),
        "{}",
        json.text()
    );
}
