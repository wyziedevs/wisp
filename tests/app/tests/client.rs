//! The test app answering in process, through `wisp::test::client`: the
//! same parser, limits, hooks and pages as on the wire, with no server.

use wisp::test::client;
use wisp::{Body, Request, Value};
use wisp_test_app::Site;

#[test]
fn pages_hooks_and_errors() {
    let mut app = client::<Site>();
    let home = app.get("/");
    assert_eq!(home.status, 200);
    assert!(
        home.text().contains("<h1>hello from init</h1>"),
        "{}",
        home.text()
    );
    assert!(
        home.text().contains("/_app/wisp.js?v="),
        "the head tags are in: {}",
        home.text()
    );
    assert_eq!(
        home.header("content-type"),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(home.header("x-app"), Some("test"));

    let missing = app.get("/nope");
    assert_eq!(
        (missing.status, missing.header("x-app")),
        (404, Some("test"))
    );
    assert!(missing.text().contains("There is nothing at this address."));

    let slash = app.get("/login/?a=1");
    assert_eq!(
        (slash.status, slash.header("location")),
        (308, Some("/login?a=1"))
    );
}

#[test]
fn sign_in_keeps_cookies() {
    let mut app = client::<Site>();
    assert_eq!(app.get("/admin").header("location"), Some("/login"));
    let login = app.post_form("/login", &[("name", "ada")]);
    assert_eq!(
        (login.status, login.header("location")),
        (303, Some("/admin"))
    );
    assert!(app.cookie("user").is_some_and(|c| c.starts_with("ada.")));
    let admin = app.get("/admin");
    assert_eq!(admin.status, 200);
    assert!(admin.text().contains("Welcome, ada"));
    // The flash from signing in shows once.
    assert!(
        admin.text().contains("<p class=\"flash\">Hello, ada!</p>"),
        "{}",
        admin.text()
    );
    assert!(!app.get("/admin").text().contains("class=\"flash\""));

    let bad = app.post_form("/login", &[("name", "a d&a")]);
    assert_eq!(bad.status, 400);
}

#[test]
fn same_limits_as_the_server() {
    let mut app = client::<Site>();
    let mut big = Request::new("POST", "/echo");
    big.body = vec![b'a'; 100 * 1024];
    assert_eq!(app.send(big).status, 413);

    let mut echo = Request::new("POST", "/echo");
    echo.body = b"hi".to_vec();
    // Framing is the host's: the body given is the body.
    echo.header("transfer-encoding", "chunked");
    echo.header("content-length", "99");
    assert_eq!(app.send(echo).text(), "2:hi");

    let mut split = Request::new("GET", "/");
    split.header("x-a", "1\r\nx-b: 2");
    assert_eq!(app.send(split).status, 400);
    assert_eq!(
        app.send(Request::new("GET", "/ HTTP/1.1\r\nx: y")).status,
        400
    );
    let mut huge = Request::new("GET", "/");
    huge.header("x-big", &"a".repeat(20 * 1024));
    assert_eq!(app.send(huge).status, 431);
}

#[test]
fn streams_and_files() {
    let mut app = client::<Site>();
    let mut events = app.get("/events");
    assert_eq!(events.header("content-type"), Some("text/event-stream"));
    assert!(matches!(events.body, Body::Stream(_)));
    for i in 0..3 {
        let chunk = app.next_chunk(&mut events).unwrap();
        assert_eq!(
            String::from_utf8(chunk).unwrap(),
            format!("data: tick {i}\ndata: line two\n\n")
        );
    }
    assert_eq!(app.next_chunk(&mut events), None);

    let js = app.get("/_app/wisp.js");
    assert_eq!(js.status, 200);
    let etag = js.header("etag").unwrap().to_string();
    let mut again = Request::new("GET", "/_app/wisp.js");
    again.header("if-none-match", &etag);
    let again = app.send(again);
    assert_eq!(
        (
            again.status,
            again.bytes().len(),
            again.header("content-type")
        ),
        (304, 0, None)
    );
}

#[test]
fn head_has_the_headers_and_no_body() {
    let mut app = client::<Site>();
    let get = app.get("/");
    let head = app.send(Request::new("HEAD", "/"));
    assert_eq!(head.status, 200);
    assert_eq!(head.header("content-type"), get.header("content-type"));
    assert!(head.bytes().is_empty());
    assert_eq!(
        head.header("content-length").map(str::to_string),
        Some(get.bytes().len().to_string())
    );
}

#[test]
fn no_content_has_no_body_or_length() {
    let mut app = client::<Site>();
    let preflight = app.send(Request::new("OPTIONS", "/echo"));
    assert_eq!(
        (
            preflight.status,
            preflight.bytes().len(),
            preflight.header("content-length")
        ),
        (204, 0, None)
    );
}

#[test]
fn a_rest_table_is_an_api() {
    let mut app = client::<Site>();
    assert_eq!(app.get("/notes").text(), "[]");
    let denied = app.post_json("/notes", r#"{"title":"Milk"}"#);
    assert_eq!(
        (denied.status, denied.header("www-authenticate")),
        (401, Some("Bearer"))
    );
    app.bearer("wisp-test-app");
    let made = app.post_json("/notes", r#"{"title":"Milk"}"#);
    assert_eq!(
        (made.status, made.header("location")),
        (201, Some("/notes/1"))
    );
    assert_eq!(made.text(), r#"{"id":1,"title":"Milk","done":false}"#);
    let bad = app.post_json("/notes", r#"{"title":""}"#);
    assert!(
        bad.status == 422 && bad.text().contains("title"),
        "{}",
        bad.text()
    );
    let done = app.patch_json("/notes/1", r#"{"done":true,"id":7}"#);
    assert_eq!(done.text(), r#"{"id":1,"title":"Milk","done":true}"#);
    assert_eq!(app.patch_json("/notes/1", r#"{"title":""}"#).status, 422);
    assert_eq!(app.patch_json("/notes/1", "[1]").status, 422);
    let put = app.put_json("/notes/1", r#"{"title":"Eggs"}"#);
    assert_eq!(put.text(), r#"{"id":1,"title":"Eggs","done":false}"#);
    assert_eq!(app.get("/notes").text(), format!("[{}]", put.text()));
    assert_eq!(app.get("/notes/x").status, 404);
    assert_eq!(app.put_json("/notes/9", r#"{"title":"x"}"#).status, 404);
    assert_eq!(app.delete("/notes/1").status, 204);
    assert_eq!(app.delete("/notes/1").status, 404);
    assert_eq!(app.get("/notes/1").status, 404);
}

/// The titles of a list of rows.
fn titles(r: &wisp::Reply) -> Vec<String> {
    let rows: Vec<Value> = r.json();
    rows.iter()
        .map(|v| v.get("title").and_then(Value::as_str).unwrap().to_string())
        .collect()
}

#[test]
fn rest_resources_filter_sort_page_and_hook() {
    let mut app = client::<Site>();
    // Several at once: the hook trims each, and Wisp stamps them.
    let made = app.post_json(
        "/tasks",
        r#"[{"title":" Tea ","points":3,"tags":["home"]},{"title":"Milk","points":1,"done":true},{"title":"Eggs","points":2,"tags":["home","shop"]}]"#,
    );
    assert_eq!(made.status, 201, "{}", made.text());
    let rows: Vec<Value> = made.json();
    assert_eq!(rows[0].get("title").and_then(Value::as_str), Some("Tea"));
    let created = rows[0].get("created_at").and_then(Value::as_str).unwrap();
    assert!(created.len() == 20 && created.ends_with('Z'), "{created}");
    let updated = rows[0].get("updated_at").and_then(Value::as_i64).unwrap();
    assert!(updated > 1_700_000_000);
    let bad = app.post_json("/tasks", r#"[{"title":"ok"},{"title":""}]"#);
    assert!(
        bad.status == 422 && bad.text().contains("[1].title"),
        "{}",
        bad.text()
    );
    let refused = app.post_json("/tasks", r#"{"title":"forbidden","points":0}"#);
    assert!(
        refused.text().contains("is not allowed"),
        "{}",
        refused.text()
    );
    assert_eq!(app.get("/tasks").header("x-total-count"), Some("3"));

    assert_eq!(titles(&app.get("/tasks?done=true")), ["Milk"]);
    assert_eq!(titles(&app.get("/tasks?points.gte=2")), ["Tea", "Eggs"]);
    assert_eq!(titles(&app.get("/tasks?tags.has=shop")), ["Eggs"]);
    assert_eq!(titles(&app.get("/tasks?title.has=EG")), ["Eggs"]);
    assert_eq!(
        titles(&app.get("/tasks?sort=-points")),
        ["Tea", "Eggs", "Milk"]
    );
    assert_eq!(
        titles(&app.get("/tasks?sort=title")),
        ["Eggs", "Milk", "Tea"]
    );
    let unknown = app.get("/tasks?color=red");
    assert_eq!(unknown.status, 400);
    assert!(
        unknown.text().contains(r#""code":"unknown_field""#),
        "{}",
        unknown.text()
    );
    assert_eq!(
        app.get("/tasks/2?fields=title,done").text(),
        r#"{"id":2,"title":"Milk","done":true}"#
    );

    let page = app.get("/tasks?limit=2");
    assert_eq!(titles(&page), ["Tea", "Milk"]);
    assert_eq!(
        page.header("link"),
        Some("</tasks?limit=2&after=2>; rel=\"next\"")
    );
    let last = app.get("/tasks?limit=2&after=2");
    assert_eq!(
        (titles(&last), last.header("link")),
        (vec!["Eggs".to_string()], None)
    );
    let sorted = app.get("/tasks?sort=title&limit=1&offset=1");
    assert_eq!(titles(&sorted), ["Milk"]);
    assert_eq!(
        sorted.header("link"),
        Some("</tasks?sort=title&limit=1&offset=2>; rel=\"next\"")
    );
    let mut lines = Request::new("GET", "/tasks?fields=title");
    lines.header("accept", "application/x-ndjson");
    assert_eq!(
        app.send(lines).text(),
        "{\"id\":1,\"title\":\"Tea\"}\n{\"id\":2,\"title\":\"Milk\"}\n{\"id\":3,\"title\":\"Eggs\"}\n"
    );

    // ETags: a copy the client has is a 304, and a write from a stale one a 412.
    let one = app.get("/tasks/1");
    let tag = one.header("etag").unwrap().to_string();
    app.header("if-none-match", &tag);
    assert_eq!(app.get("/tasks/1").status, 304);
    app.header("if-match", &tag);
    let changed = app.patch_json("/tasks/1", r#"{"points":5,"tags":null}"#);
    assert_eq!(changed.status, 200, "{}", changed.text());
    let (was, now): (Value, Value) = (one.json(), changed.json());
    assert_eq!(now.get("created_at"), was.get("created_at"));
    assert_eq!(now.get("tags"), Some(&Value::Null));
    app.header("if-match", &tag);
    let stale = app.patch_json("/tasks/1", r#"{"points":6}"#);
    assert_eq!(stale.status, 412);
    assert!(stale.text().contains(r#""code":"changed""#));
    let hooked = app.patch_json("/tasks/1", r#"{"points":101}"#);
    assert!(hooked.status == 422 && hooked.text().contains("100 at most"));

    // A missing row is JSON for an API client, and problem+json if asked.
    let missing = app.get("/tasks/99");
    assert_eq!(missing.status, 404);
    assert!(
        missing.text().contains(r#""code":"not_found""#),
        "{}",
        missing.text()
    );
    app.header("accept", "application/problem+json");
    let problem = app.get("/tasks/99");
    assert_eq!(
        problem.header("content-type"),
        Some("application/problem+json")
    );
    assert!(
        problem
            .text()
            .starts_with(r#"{"type":"about:blank","title":"Not Found""#)
    );

    // The same Idempotency-Key gets the first answer again.
    app.header("idempotency-key", "k1");
    let first = app.post_json("/tasks", r#"{"title":"Once","points":0}"#);
    app.header("idempotency-key", "k1");
    let again = app.post_json("/tasks", r#"{"title":"Once","points":0}"#);
    assert_eq!((first.status, again.text()), (201, first.text()));
    assert_eq!(again.header("idempotent-replayed"), Some("true"));
    app.header("idempotency-key", "k1");
    assert_eq!(
        app.post_json("/tasks", r#"{"title":"Twice","points":0}"#)
            .status,
        422
    );
    assert_eq!(app.get("/tasks").header("x-total-count"), Some("4"));

    // Deleting takes the admin key; the after_delete hook sees the row.
    assert_eq!(app.delete("/tasks/2").status, 401);
    app.bearer("wisp-test-app");
    let gone = app.delete("/tasks/2");
    assert_eq!((gone.status, gone.header("x-deleted")), (204, Some("Milk")));
}

#[test]
fn a_resource_under_another_holds_its_own() {
    let mut app = client::<Site>();
    let pen = app.post_json("/users/1/items", r#"{"name":"pen"}"#);
    assert_eq!(pen.status, 201, "{}", pen.text());
    let pen: Value = pen.json();
    assert_eq!(pen.get("user").and_then(Value::as_i64), Some(1));
    let id = pen.get("id").and_then(Value::as_i64).unwrap();
    assert!(id > 1000, "random ids: {id}");
    let cup = app.post_json("/users/2/items", r#"{"name":"cup","user":1}"#);
    assert!(cup.text().contains(r#""user":2"#), "{}", cup.text());
    let mine: Vec<Value> = app.get("/users/1/items").json();
    assert_eq!(mine, [pen]);
    assert_eq!(app.get(&format!("/users/1/items/{id}")).status, 200);
    assert_eq!(app.get(&format!("/users/2/items/{id}")).status, 404);
    assert_eq!(app.delete(&format!("/users/2/items/{id}")).status, 404);
}

#[test]
fn webhooks_and_lines() {
    let mut app = client::<Site>();
    let body = r#"{"action":"opened"}"#;
    let sign = |body: &str| {
        format!(
            "sha256={}",
            wisp::hex(&wisp::hmac_sha256("wisp-test-app", body))
        )
    };
    let mut hook = Request::new("POST", "/webhook");
    hook.header("x-hub-signature-256", &sign(body));
    hook.body = body.as_bytes().to_vec();
    assert_eq!(app.send(hook).text(), "\"ok\"");
    let mut forged = Request::new("POST", "/webhook");
    forged.header("x-hub-signature-256", &sign("{}"));
    forged.body = body.as_bytes().to_vec();
    let forged = app.send(forged);
    assert_eq!(forged.status, 401);
    assert!(forged.text().contains("bad_signature"), "{}", forged.text());

    let mut lines = app.get("/lines");
    assert_eq!(lines.header("content-type"), Some("application/x-ndjson"));
    for n in 1..=3 {
        assert_eq!(
            app.next_chunk(&mut lines),
            Some(format!("{n}\n").into_bytes())
        );
    }
    assert_eq!(app.next_chunk(&mut lines), None);
}

#[test]
fn handlers_with_an_id_serve_the_member_route() {
    let mut app = client::<Site>();
    assert_eq!(app.post_form("/tags", &[("name", "red")]).text(), "1");
    assert_eq!(app.get("/tags").text(), r#"[{"id":1,"value":"red"}]"#);
    let one = app.get("/tags/1");
    assert_eq!(one.text(), r#"{"id":1,"value":"red"}"#);
    assert_eq!(one.header("x-tags"), Some("yes"));
    assert_eq!(app.get("/tags/2").status, 404);
    assert_eq!(app.post_form("/tags/1", &[]).status, 405);
    assert_eq!(app.delete("/tags/1").status, 204);
    assert_eq!(app.delete("/tags/1").status, 404);
}

#[test]
fn action_forms_post_check_and_keep_input() {
    let mut app = client::<Site>();
    let page = app.get("/todos");
    let html = page.text();
    let head = &html[..html.find("</head>").unwrap()];
    assert!(head.contains("<title>Todos 0</title>"), "{html}");
    assert!(
        html.contains("<form action=\"?/add\" method=\"post\">"),
        "{html}"
    );
    assert!(html.contains("<svg><title>icon</title></svg>"), "{html}");
    assert!(html.contains("<div class=\"box\">boxed<form"), "{html}");
    assert!(html.contains("<p class=\"problem\"></p>"), "{html}");
    let long = app.post_form(
        "/todos?/add",
        &[("text", "far <too> long"), ("secret", "s")],
    );
    let html = long.text();
    assert_eq!(long.status, 422);
    assert!(
        html.contains("<input name=\"text\" value=\"far &lt;too&gt; long\">"),
        "{html}"
    );
    assert!(
        html.contains("<input name=\"secret\" type=\"password\">"),
        "{html}"
    );
    assert!(
        html.contains("<p class=\"problem\">must have at most 10 characters</p>"),
        "{html}"
    );
    assert!(
        html.contains("<form action=\"?/add\" method=\"post\"><input name=\"text\"></form>"),
        "a component keeps nothing: {html}"
    );
    let ok = app.post_form("/todos?/add", &[("text", "milk")]);
    let one =
        "<li>milk<form method=\"post\"><button formaction=\"?/remove&id=1\">x</button></form></li>";
    assert!(ok.status == 200 && ok.text().contains(one), "{}", ok.text());
    assert!(ok.text().contains("<input name=\"text\">"), "{}", ok.text());
    assert_eq!(app.post_form("/todos?/add", &[("text", "")]).status, 422);
    let gone = app.post_form("/todos?/remove&id=1", &[]);
    assert!(
        gone.status == 200 && !gone.text().contains("<li>"),
        "{}",
        gone.text()
    );
}
