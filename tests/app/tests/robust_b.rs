//! Request and response data at their edges: what browsers send as an
//! email, JSON that is not JSON, conditional writes, idempotency keys.

use wisp::test::client;
use wisp_test_app::Site;

#[test]
fn an_email_a_browser_takes_is_taken() {
    let mut app = client::<Site>();
    // `<input type="email">` takes `a@b`, and Firefox sends a Unicode domain.
    for ok in ["a@b", "a@bücher.de", "x.y+z@mail.example.org"] {
        let r = app.post_form("/t/sugar?/join", &[("email", ok)]);
        assert_eq!(r.status, 303, "{ok}");
    }
    for bad in ["a@b.", "a@-b", "a b@c", "ü@b"] {
        let r = app.post_form("/t/sugar?/join", &[("email", bad)]);
        assert_eq!(r.status, 422, "{bad}");
    }
}

#[test]
fn json_that_is_not_json_is_a_400_everywhere() {
    let mut app = client::<Site>();
    // An action's parameters read from a JSON body: not each one missing.
    let action = app.post_json("/t/sugar?/join", r#"{"email":"#);
    assert_eq!(action.status, 400, "{}", action.text());
    assert!(action.text().contains("Invalid JSON"), "{}", action.text());
    // `body: T` of a REST type: the same 400, and a 422 by field for JSON
    // that is not a `T`.
    let rest = app.post_json("/tasks", r#"{"title":"#);
    assert_eq!(rest.status, 400);
    assert!(
        rest.text().contains(r#""code":"bad_request""#),
        "{}",
        rest.text()
    );
    let invalid = app.post_json("/tasks", r#"{"title":"","points":-1}"#);
    assert_eq!(invalid.status, 422);
    assert!(
        invalid
            .text()
            .starts_with(r#"{"status":422,"code":"invalid","error":"#),
        "{}",
        invalid.text()
    );
    assert!(
        invalid.text().contains(r#""errors":{"title":"#),
        "{}",
        invalid.text()
    );
}

#[test]
fn if_match_compares_strongly() {
    let mut app = client::<Site>();
    let made = app.post_json("/tasks", r#"{"title":"Tea","points":1}"#);
    assert_eq!(made.status, 201);
    let at = made.header("location").unwrap().to_string();
    let tag = app.get(&at).header("etag").unwrap().to_string();
    app.header("if-match", &format!("W/{tag}"));
    let weak = app.put_json(&at, r#"{"title":"Weak","points":1}"#);
    assert_eq!(weak.status, 412, "a weak tag promises no bytes");
    app.header("if-match", &tag);
    let strong = app.put_json(&at, r#"{"title":"Strong","points":1}"#);
    assert_eq!(strong.status, 200, "{}", strong.text());

    // A PATCH of many names the type does not have is read once.
    let mut many: String = (0..40_000).map(|i| format!("\"x{i}\":1,")).collect();
    many = format!("{{{many}\"done\":true}}");
    let patched = app.patch_json(&at, &many);
    assert_eq!(patched.status, 200);
    assert!(patched.text().contains(r#""done":true"#));
}

#[test]
fn one_visitor_never_gets_another_visitors_answer() {
    let mut app = client::<Site>();
    let body = r#"{"title":"Mine","points":0}"#;
    app.header("idempotency-key", "same");
    app.header("cookie", "session=ann");
    let ann = app.post_json("/tasks", body);
    app.header("idempotency-key", "same");
    app.header("cookie", "session=bob");
    let bob = app.post_json("/tasks", body);
    assert_eq!((ann.status, bob.status), (201, 201));
    assert_eq!(bob.header("idempotent-replayed"), None);
    assert_ne!(ann.header("location"), bob.header("location"));
    app.header("idempotency-key", "same");
    app.header("cookie", "session=ann");
    let again = app.post_json("/tasks", body);
    assert_eq!(again.header("idempotent-replayed"), Some("true"));
    assert_eq!(again.text(), ann.text());
}
