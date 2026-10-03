//! `#[remote]` functions (`src/remote.rs`, and a page's block in
//! `src/routes/remote`), called as the browser's `remote.js` calls them.

use wisp::Value;
use wisp::test::{Client, client};
use wisp_test_app::Site;

/// Where `remote.js` sends a call of `name`.
fn path(app: &mut Client<Site>, name: &str) -> String {
    let js = app.get("/_app/c/remote.js");
    assert_eq!(js.status, 200);
    let text = js.text();
    let at = format!("export const {name} = (...a) => call(\"");
    let start = text.find(&at).unwrap_or_else(|| panic!("{name} in {text}")) + at.len();
    text[start..start + text[start..].find('"').unwrap()].to_string()
}

#[test]
fn a_call_is_a_post_of_its_arguments_by_name() {
    let mut app = client::<Site>();
    let add = path(&mut app, "add");
    assert!(add.starts_with("/_app/r/"), "{add}");
    let r = app.post_json(&add, r#"{"a": 2, "b": 40}"#);
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.text(), "42");
    // A struct, read with its FromJson; answered with its Json.
    let swap = path(&mut app, "swap");
    let r = app.post_json(&swap, r#"{"pair": {"a": 1, "b": 2}}"#);
    assert_eq!(r.text(), r#"{"a":2,"b":1}"#);
    // A page's own, from its block, which may be async.
    let twice = path(&mut app, "twice");
    assert_eq!(app.post_json(&twice, r#"{"n": 21}"#).text(), "42");
}

#[test]
fn arguments_are_checked_by_field() {
    let mut app = client::<Site>();
    let add = path(&mut app, "add");
    let r = app.post_json(&add, r#"{"a": "two"}"#);
    assert_eq!(r.status, 422, "{}", r.text());
    let v: Value = r.json();
    let errors = v.get("errors").unwrap();
    assert!(
        errors.get("a").is_some() && errors.get("b").is_some(),
        "{v:?}"
    );
    let shout = path(&mut app, "shout");
    assert_eq!(
        app.post_json(&shout, r#"{"word": "hey"}"#).text(),
        "\"HEY\""
    );
    let r = app.post_json(&shout, r#"{"word": "far too long"}"#);
    assert_eq!(r.status, 422, "{}", r.text());
    assert_eq!(app.post_json(&add, "[1, 2]").status, 400);
    assert_eq!(app.post_json(&add, "{").status, 400);
}

#[test]
fn errors_and_redirects_answer_as_the_browser_reads_them() {
    let mut app = client::<Site>();
    let missing = path(&mut app, "missing");
    let r = app.post_json(&missing, r#"{"id": 7}"#);
    assert_eq!(r.status, 404);
    let v: Value = r.json();
    assert_eq!(v.get("error").and_then(Value::as_str), Some("No thing 7"));
    let leave = path(&mut app, "leave");
    app.header("x-wisp", "1");
    let r = app.post_json(&leave, "");
    assert_eq!(r.status, 200);
    assert_eq!(r.header("x-wisp-location"), Some("/"));
    assert_eq!(app.post_json(&leave, "").status, 303);
    // Not one: a 404, as JSON.
    let r = app.post_json("/_app/r/0000000000000000", "{}");
    assert_eq!(r.status, 404);
    assert!(r.header("content-type").unwrap().contains("json"));
}

#[test]
fn hooks_and_the_origin_check_apply() {
    let mut app = client::<Site>();
    let whoami = path(&mut app, "whoami");
    // `before` ran: its header is on the answer.
    let r = app.post_json(&whoami, "");
    assert_eq!(r.status, 404, "no one signed in: None");
    assert_eq!(r.header("x-app"), Some("test"));
    app.header("origin", "https://evil.example");
    assert_eq!(app.post_json(&whoami, "").status, 403);
    // A POST function is not a GET one.
    assert_eq!(app.get(&whoami).status, 405);
}

#[test]
fn a_get_function_takes_its_query_and_is_cached() {
    let mut app = client::<Site>();
    let greet = path(&mut app, "greet");
    let r = app.get(&format!("{greet}?name=%22Ada%22&title=Dr"));
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.text(), "\"Hello, Dr Ada\"");
    let tag = r.header("etag").unwrap().to_string();
    app.header("if-none-match", &tag);
    let again = app.get(&format!("{greet}?name=%22Ada%22&title=Dr"));
    assert_eq!(again.status, 304);
    assert_eq!(
        app.get(&format!("{greet}?name=Ada")).text(),
        "\"Hello, Ada\""
    );
    assert_eq!(app.post_json(&greet, "{}").status, 405);
}

#[test]
fn scripts_call_them_without_an_import() {
    let mut app = client::<Site>();
    let page = app.get("/remote");
    assert_eq!(page.status, 200);
    let text = page.text();
    let at = text.find("/_app/c/t").unwrap();
    let url = &text[at..at + text[at..].find('"').unwrap()];
    let module = app.get(url);
    let js = module.text();
    assert!(
        js.contains("import { twice, greet, missing } from \"/_app/c/remote.js?v="),
        "{js}"
    );
    let lib = app.get("/_app/c/lib/calls.js");
    assert!(
        lib.text()
            .starts_with("import { add } from \"/_app/c/remote.js?v="),
        "{}",
        lib.text()
    );
}

/// In a browser: the calls answer, reject with `{ status, message }`, and
/// a lib file's import works.
#[cfg(feature = "browser")]
#[test]
fn a_browser_calls_them() {
    let mut b = wisp::browser!(Site);
    b.goto("/remote");
    for (button, out) in [
        ("Twice", "42"),
        ("Sum", "5"),
        ("Missing", "404 No thing 7"),
        ("Greet", "Hello, Dr Ada"),
    ] {
        b.click(&format!("text={button}"));
        b.wait(&format!("text={out}"));
        assert_eq!(b.text("output"), out);
    }
}
