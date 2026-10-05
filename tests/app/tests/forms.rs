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
        "<input aria-label=\"title\" name=\"title\" value=\" \" required minlength=\"1\" pattern=\"[\\s\\S]{0,20}\" aria-invalid=\"true\"><small class=\"problem\">is required</small>",
        "<textarea aria-label=\"body\" name=\"body\" required minlength=\"3\" aria-invalid=\"true\">hi</textarea><small class=\"problem\">must have at least 3 characters</small>",
        "<option value=\"a\" selected>A</option><option value=\"b\">B</option></select>",
        "<input aria-label=\"stars\" name=\"stars\" value=\"x\" required aria-invalid=\"true\"><small class=\"problem\">expected a whole number</small>",
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

    // Refused, a checkbox shows what was sent, not what was saved:
    // unticked when it was left out, ticked when it was sent.
    let unticked = [
        ("title", ""),
        ("body", "Longer"),
        ("kind", "a"),
        ("stars", "5"),
    ];
    let html = app.post_form("/t/forms", &unticked).text().to_string();
    let draft = "<input aria-label=\"draft\" name=\"draft\" type=\"checkbox\"";
    assert!(html.contains(&format!("{draft}>")), "{html}");
    let ticked = [("title", ""), ("draft", "on")];
    let html = app.post_form("/t/forms", &ticked).text().to_string();
    assert!(html.contains(&format!("{draft} checked>")), "{html}");

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
        "<input aria-label=\"a\" name=\"a\" required minlength=\"2\" value=\"x\" aria-invalid=\"true\"><small class=\"problem\">must have at least 2 characters</small>",
        // Shown where the page puts it, and so not after its input.
        "<input aria-label=\"n\" name=\"n\" type=\"number\" required min=\"5\" value=\"1\" aria-invalid=\"true\"><input",
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

/// `src/routes/t/forms2`: two forms that share field names, a box that
/// must be ticked, a radio group, a `<select multiple>` and a file. Only
/// the refused form shows what was sent; each problem is beside its input,
/// which says so (`aria-invalid`, `aria-describedby` when it has an id).
#[test]
fn only_the_refused_form_keeps_what_was_sent() {
    let mut app = client::<Site>();
    let page = app.get("/t/forms2").text().to_string();
    for want in [
        "<input id=\"name\" aria-label=\"name\" name=\"name\" value=\"Ada\" required minlength=\"2\">",
        "<textarea id=\"note\" aria-label=\"note\" name=\"note\">own</textarea>",
        "<option value=\"a\">A</option><option value=\"b\" selected>B</option>",
        "<input type=\"radio\" name=\"size\" value=\"m\" checked>",
        "<input type=\"checkbox\" name=\"agree\" id=\"agree\">",
        "<input id=\"name2\" aria-label=\"name2\" name=\"name\" value=\"Bob\">",
        "<input aria-label=\"agree2\" type=\"checkbox\" name=\"agree\" checked>",
    ] {
        assert!(page.contains(want), "{want} in {page}");
    }
    assert!(
        !page.contains("<small") && !page.contains("aria-invalid"),
        "{page}"
    );

    // The first form refused: it keeps every kind of field as sent, the
    // second shows its own values still.
    let fields = [
        ("name", "A"),
        ("note", "typed <b>"),
        ("kind", "a"),
        ("tags", "x"),
        ("tags", "z"),
        ("size", "s"),
    ];
    let bad = app.post_form("/t/forms2?/join", &fields);
    assert_eq!(bad.status, 422);
    let html = bad.text();
    for want in [
        "<input id=\"name\" aria-label=\"name\" name=\"name\" value=\"A\" required minlength=\"2\" aria-invalid=\"true\" aria-describedby=\"name-problem\">\
         <small class=\"problem\" id=\"name-problem\">must have at least 2 characters</small>",
        "<textarea id=\"note\" aria-label=\"note\" name=\"note\">typed &lt;b&gt;</textarea>",
        "<option value=\"a\" selected>A</option><option value=\"b\">B</option></select>\n<select aria-label=\"tags\"",
        "<option value=\"x\" selected>X</option><option value=\"y\">Y</option><option value=\"z\" selected>Z</option>",
        "<input type=\"radio\" name=\"size\" value=\"s\" checked> S</label>",
        "<input type=\"radio\" name=\"size\" value=\"m\"> M</label>",
        // (The rules come first: the action, which asks for the tick, has
        // not run.)
        "<input type=\"checkbox\" name=\"agree\" id=\"agree\"> I agree</label>",
        "<input id=\"name2\" aria-label=\"name2\" name=\"name\" value=\"Bob\">",
        "<textarea aria-label=\"note2\" name=\"note\">other own</textarea>",
        "<option value=\"a\" selected>A</option><option value=\"b\">B</option></select>\n<input aria-label=\"agree2\"",
        "<input aria-label=\"agree2\" type=\"checkbox\" name=\"agree\" checked>",
    ] {
        assert!(html.contains(want), "{want} in {html}");
    }
    assert_eq!(html.matches("aria-invalid").count(), 1, "{html}");

    // The rules pass and the action asks for the tick: the problem of the
    // box that must be ticked is beside it, and the rest is kept.
    let fields = [("name", "Al"), ("note", "n"), ("kind", "a"), ("tags", "y")];
    let bad = app.post_form("/t/forms2?/join", &fields);
    assert_eq!(bad.status, 422);
    let html = bad.text();
    for want in [
        "<input type=\"checkbox\" name=\"agree\" id=\"agree\" aria-invalid=\"true\"><small class=\"problem\">Tick to accept the terms</small> I agree</label>",
        "<input id=\"name\" aria-label=\"name\" name=\"name\" value=\"Al\" required minlength=\"2\">",
        "<option value=\"x\">X</option><option value=\"y\" selected>Y</option><option value=\"z\">Z</option>",
        // No radio sent: none ticked.
        "<input type=\"radio\" name=\"size\" value=\"m\"> M</label>",
    ] {
        assert!(html.contains(want), "{want} in {html}");
    }
    assert_eq!(html.matches("aria-invalid").count(), 1, "{html}");
    assert_eq!(html.matches("<small").count(), 1, "{html}");

    // The second form refused, with two problems of one field: both are
    // shown, the first with the id; the first form is its own.
    let bad = app.post_form(
        "/t/forms2?/other",
        &[("name", "twice"), ("note", "hi"), ("kind", "b")],
    );
    assert_eq!(bad.status, 422);
    let html = bad.text();
    for want in [
        "<input id=\"name2\" aria-label=\"name2\" name=\"name\" value=\"twice\" aria-invalid=\"true\" aria-describedby=\"name2-problem\">\
         <small class=\"problem\" id=\"name2-problem\">first</small><small class=\"problem\">second</small>",
        "<textarea aria-label=\"note2\" name=\"note\">hi</textarea>",
        "<option value=\"a\">A</option><option value=\"b\" selected>B</option></select>\n<input aria-label=\"agree2\"",
        "<input aria-label=\"agree2\" type=\"checkbox\" name=\"agree\">",
        "<input id=\"name\" aria-label=\"name\" name=\"name\" value=\"Ada\" required minlength=\"2\">",
        "<textarea id=\"note\" aria-label=\"note\" name=\"note\">own</textarea>",
        "<input type=\"radio\" name=\"size\" value=\"m\" checked>",
        "<input type=\"checkbox\" name=\"agree\" id=\"agree\">",
    ] {
        assert!(html.contains(want), "{want} in {html}");
    }
    assert_eq!(html.matches("aria-invalid").count(), 1, "{html}");

    // What was sent is text wherever it goes back.
    let evil = "\"><script>alert(1)</script>";
    let bad = app.post_form(
        "/t/forms2?/join",
        &[
            ("name", evil),
            ("note", evil),
            ("kind", evil),
            ("tags", evil),
            ("size", evil),
        ],
    );
    assert_eq!(bad.status, 422);
    let html = bad.text();
    assert!(!html.contains("<script>alert"), "{html}");
    assert!(
        html.contains("value=\"&quot;&gt;&lt;script&gt;alert(1)&lt;/script&gt;\""),
        "{html}"
    );
    assert!(
        html.contains("<option value=\"a\">A</option><option value=\"b\">B</option>"),
        "no choice"
    );

    // Sent as multipart (a file along), the text is kept the same way.
    const B: &str = "----wisp-test-boundary";
    let part = |name: &str, value: &str| {
        format!("--{B}\r\ncontent-disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
    };
    let mut body = part("name", "M") + &part("note", "multi") + &part("kind", "a");
    body += &format!(
        "--{B}\r\ncontent-disposition: form-data; name=\"doc\"; filename=\"a.txt\"\r\ncontent-type: text/plain\r\n\r\nhello\r\n--{B}--\r\n"
    );
    let mut req = wisp::Request::new("POST", "/t/forms2?/join");
    req.header("accept", "text/html");
    req.header(
        "content-type",
        &format!("multipart/form-data; boundary={B}"),
    );
    req.body = body.into_bytes();
    let bad = app.send(req);
    assert_eq!(bad.status, 422);
    let html = bad.text();
    assert!(
        html.contains("name=\"name\" value=\"M\" required"),
        "{html}"
    );
    assert!(html.contains(">multi</textarea>"), "{html}");
    assert!(!html.contains("hello"), "the file never goes back: {html}");
}

/// The other requests a page with actions gets: HEAD and OPTIONS, methods
/// it does not take (405 with `allow`), a post to no such action (404),
/// one from another site (403), one too big (413), one whose body is not
/// a form (its fields missing), a redirect after a post (303, or 200 with
/// `x-wisp-location` for wisp.js), and bad UTF-8 kept as text.
#[test]
fn what_else_an_action_page_answers() {
    let mut app = client::<Site>();
    let head = app.send(wisp::Request::new("HEAD", "/t/forms2"));
    assert_eq!(head.status, 200);
    assert!(head.text().is_empty(), "{}", head.text());
    // (This app's `before` hook answers OPTIONS itself, a bare 204; the
    // route's own `allow` shows on a 405.)
    let options = app.send(wisp::Request::new("OPTIONS", "/t/forms2"));
    assert_eq!(options.status, 204);
    assert!(options.text().is_empty());
    for method in ["PUT", "PATCH", "DELETE"] {
        let r = app.send(wisp::Request::new(method, "/t/forms2?/join"));
        assert_eq!(r.status, 405, "{method}");
        let allow = r.header("allow").unwrap_or("").to_string();
        assert!(
            allow.contains("GET") && allow.contains("POST") && allow.contains("OPTIONS"),
            "{method}: {allow}"
        );
    }
    // A GET with an action in its query is the page.
    assert_eq!(app.get("/t/forms2?/join").status, 200);
    let gone = app.post_form("/t/forms2?/nope", &[("name", "x")]);
    assert_eq!(gone.status, 404);
    assert!(gone.text().contains("nope"), "{}", gone.text());

    // Cross-site: refused before the action runs.
    let fields = [
        ("name", "Al"),
        ("note", "n"),
        ("kind", "a"),
        ("agree", "on"),
    ];
    for (header, value) in [
        ("origin", "https://evil.example"),
        ("sec-fetch-site", "cross-site"),
        ("sec-fetch-site", "same-site"),
    ] {
        // (A header given this way goes with the next request only.)
        app.header(header, value);
        assert_eq!(
            app.post_form("/t/forms2?/join", &fields).status,
            403,
            "{header}: {value}"
        );
    }
    app.header("sec-fetch-site", "same-origin");
    assert_eq!(app.post_form("/t/forms2?/join", &fields).status, 303);

    // Too big for the page's limit; not a form at all.
    let big = "x".repeat(2 * 1024 * 1024);
    assert_eq!(
        app.post_form("/t/forms2?/join", &[("note", &big)]).status,
        413
    );
    let mut req = wisp::Request::new("POST", "/t/forms2?/join");
    req.header("content-type", "application/x-www-form-urlencoded");
    req.body = b"{\"name\": \"Al\"}".to_vec();
    let odd = app.send(req);
    assert_eq!(odd.status, 400, "{}", odd.text());
    assert!(
        odd.text().contains("missing form field `name`"),
        "{}",
        odd.text()
    );

    // The redirect after a post, and how wisp.js is told of it.
    let done = app.post_form("/t/forms2?/join", &fields);
    assert_eq!((done.status, done.location()), (303, Some("/t/forms2")));
    app.header("x-wisp", "1");
    let done = app.post_form("/t/forms2?/join", &fields);
    assert_eq!(done.status, 200);
    assert_eq!(done.header("x-wisp-location"), Some("/t/forms2"));

    // Bad UTF-8 in a value: kept, as text.
    let mut req = wisp::Request::new("POST", "/t/forms2?/other");
    req.header("content-type", "application/x-www-form-urlencoded");
    req.header("accept", "text/html");
    req.body = b"name=a%FFb%00c&note=%E2%82&kind=a".to_vec();
    let bad = app.send(req);
    assert_eq!(bad.status, 422, "{}", bad.text());
    assert!(
        bad.text().contains("value=\"a\u{FFFD}b\0c\""),
        "{}",
        bad.text()
    );
}
