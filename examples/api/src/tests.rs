//! The API answering in process, through `wisp::test::client`: the same
//! routing, hooks and errors as on the wire, with no port. `cargo test`.

use super::App;
use wisp::test::client;
use wisp::{FromJson, Request, Value};

#[derive(FromJson, Debug)]
struct Note {
    id: u64,
    title: String,
    tags: Vec<String>,
    done: bool,
}

#[test]
fn notes() {
    let mut app = client::<App>();
    app.bearer("dev-key");

    let made = app.post_json("/api/notes", r#"{"title": "Buy tea", "tags": ["home"]}"#);
    assert_eq!(made.status, 201, "{}", made.text());
    let note: Note = made.json();
    assert_eq!(
        (note.title.as_str(), &note.tags[..], note.done),
        ("Buy tea", &["home".to_string()][..], false)
    );

    let found: Vec<Note> = app.get("/api/notes?q=TEA").json();
    assert!(found.iter().any(|n| n.id == note.id));

    let url = format!("/api/notes/{}", note.id);
    let changed: Note = app.patch_json(&url, r#"{"done": true}"#).json();
    assert!(changed.done && changed.title == "Buy tea");
    assert_eq!(app.get(&url).json::<Note>().id, note.id);
    assert_eq!(app.delete(&url).status, 204);
    assert_eq!(app.get(&url).status, 404);
    assert_eq!(app.delete(&url).status, 404);
}

#[test]
fn problems_are_json() {
    let mut app = client::<App>();

    // Changes need the key.
    let anonymous = app.post_json("/api/notes", r#"{"title": "x"}"#);
    assert_eq!(anonymous.status, 401);
    assert_eq!(anonymous.header("www-authenticate"), Some("Bearer"));
    assert_eq!(anonymous.header("content-type"), Some("application/json"));

    app.bearer("dev-key");
    let invalid = app.post_json("/api/notes", r#"{"title": "", "tags": 3}"#);
    assert_eq!(invalid.status, 422);
    let errors = invalid.json::<Value>();
    let errors = errors.get("errors").unwrap();
    assert_eq!(
        errors.get("title").and_then(Value::as_str),
        Some("must have at least 1 character")
    );
    assert_eq!(
        errors.get("tags").and_then(Value::as_str),
        Some("expected an array, found a number")
    );

    let broken = app.post_json("/api/notes", "{\"title\": ");
    assert_eq!(broken.status, 400);
    assert!(
        broken
            .text()
            .contains("Invalid JSON: expected a value, found the end at line 1, column 11"),
        "{}",
        broken.text()
    );

    let missing = app.get("/api/nope");
    assert_eq!(
        (
            missing.status,
            missing.json::<Value>().get("error").and_then(Value::as_str)
        ),
        (404, Some("There is nothing at this address."))
    );
    assert_eq!(app.get("/api/notes/abc").status, 404, "ids are numbers");

    let wrong = app.send(Request::new("PUT", "/api/notes"));
    assert_eq!(
        (wrong.status, wrong.header("allow")),
        (405, Some("GET, HEAD, POST, OPTIONS"))
    );
}

#[test]
fn other_sites_and_tools() {
    let mut app = client::<App>();
    let mut preflight = Request::new("OPTIONS", "/api/notes");
    preflight.header("origin", "https://example.com");
    preflight.header("access-control-request-method", "POST");
    let ok = app.send(preflight);
    assert_eq!(ok.status, 204);
    assert_eq!(ok.header("access-control-allow-origin"), Some("*"));
    assert_eq!(ok.header("access-control-allow-methods"), Some("POST"));

    let options = app.send(Request::new("OPTIONS", "/api/notes"));
    assert_eq!(
        (options.status, options.header("allow")),
        (204, Some("GET, HEAD, POST, OPTIONS"))
    );

    assert_eq!(app.get("/healthz").text(), "\"ok\"");
    let spec = app.get("/_wisp/openapi.json").json::<Value>();
    let note = spec
        .get("components")
        .and_then(|c| c.get("schemas"))
        .and_then(|s| s.get("NewNote"));
    assert!(note.is_some(), "{}", wisp::to_json(&spec));
    assert!(
        spec.get("paths")
            .and_then(|p| p.get("/api/notes/{id}"))
            .is_some()
    );
    assert!(app.get("/_wisp/docs").text().contains("<title>API</title>"));
}

#[test]
fn events_as_notes_change() {
    let mut app = client::<App>();
    app.bearer("dev-key");
    let mut events = app.get("/api/events");
    assert_eq!(events.header("content-type"), Some("text/event-stream"));
    app.post_json("/api/notes", r#"{"title": "Heard live"}"#);
    // Tests running beside this one add notes too.
    loop {
        let text = String::from_utf8(app.next_chunk(&mut events).unwrap()).unwrap();
        assert!(text.starts_with("data: {\"id\":"), "{text}");
        if text.contains("\"title\":\"Heard live\"") {
            break;
        }
    }
}
