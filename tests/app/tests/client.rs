//! The test app answering in process, through `wisp::test::client`: the
//! same parser, limits, hooks and pages as on the wire, with no server.

mod common;

use common::{MULTIPART, multipart};
use wisp::test::client;
use wisp::{Body, Request, Value};
use wisp_test_app::Site;

/// The component workshop (dev builds): every component, stories from
/// `Card.stories.wisp` rendered by the server with their props from the
/// query, a default story for a component that needs nothing, and a note
/// for one that needs a story.
#[test]
fn the_workshop_renders_stories() {
    let mut app = client::<Site>();
    let index = app.get("/_wisp/components");
    if !cfg!(debug_assertions) {
        assert_eq!(index.status, 404, "dev builds only");
        return;
    }
    let text = index.text();
    assert!(
        text.contains("src/components/Card.wisp") && text.contains("2 stories"),
        "{text}"
    );
    let page = app.get("/_wisp/components/Card/featured?title=Mint");
    let text = page.text();
    assert!(
        text.contains("name=\"title\" data-set value=\"Mint\""),
        "{text}"
    );
    assert!(text.contains("name=\"featured\" checked"), "{text}");
    assert!(
        text.contains("/_wisp/components/Card/featured/frame?title=Mint"),
        "{text}"
    );
    let frame = app.get("/_wisp/components/Card/featured/frame");
    let text = frame.text();
    assert!(
        text.contains("<h2>Tea ★</h2>")
            && text.contains("3 items")
            && text.contains("A pot for two."),
        "{text}"
    );
    assert!(
        text.contains("/_app/wisp.js?v="),
        "in the app's shell: {text}"
    );
    let text = app
        .get("/_wisp/components/Card/featured/frame?title=Mint&featured=false&count=9")
        .text()
        .to_string();
    assert!(
        text.contains("<h2>Mint</h2>") && text.contains("9 items"),
        "{text}"
    );
    let text = app
        .get("/_wisp/components/Card/empty/frame")
        .text()
        .to_string();
    assert!(
        text.contains("<h2>Nothing yet</h2>") && text.contains("0 items"),
        "{text}"
    );
    let text = app
        .get("/_wisp/components/Tally/default/frame")
        .text()
        .to_string();
    assert!(text.contains("class=\"tally\""), "{text}");
    let text = app.get("/_wisp/components/Table").text().to_string();
    assert!(text.contains("Add Table.stories.wisp beside it"), "{text}");
    assert_eq!(app.get("/_wisp/components/Card/nope").status, 404);
    assert_eq!(app.get("/_wisp/components/Nope/frame/x/y").status, 404);
}

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
    assert!(missing.text().contains("Not Found"));
    // Where only endpoints are, even a browser's miss is JSON.
    let gone = app.get("/tasks/1/nothing/here");
    assert_eq!(
        (gone.status, gone.header("content-type")),
        (404, Some("application/json"))
    );
    assert!(
        gone.text().contains(r#""error":"Not Found""#),
        "{}",
        gone.text()
    );

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
    let list = app.get("/tasks");
    assert_eq!(list.header("x-total-count"), Some("3"));
    assert_eq!(
        list.header("vary"),
        Some("accept"),
        "JSON or NDJSON by accept"
    );

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
        html.contains(
            "<input name=\"text\" required minlength=\"1\" pattern=\"[\\s\\S]{0,10}\" value=\"far &lt;too&gt; long\">"
        ),
        "{html}"
    );
    assert!(
        html.contains("<input name=\"secret\" type=\"password\">"),
        "{html}"
    );
    assert!(
        html.contains(
            "<p class=\"problem\"><small class=\"problem\">must have at most 10 characters</small></p>"
        ),
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

/// A multipart post of one file, as a browser sends `<input type="file">`.
fn upload(target: &str, field: &str, bytes: &[u8]) -> Request {
    let mut req = Request::new("POST", target);
    req.header("content-type", MULTIPART);
    req.body = multipart(&[(field, Some("a"), bytes)]);
    req
}

#[test]
fn members_sign_in_and_upload_a_picture() {
    let mut app = client::<Site>();
    // Signed out, a members' page sends the visitor to sign in.
    let away = app.get("/me");
    assert_eq!(
        (away.status, away.header("location")),
        (303, Some("/login"))
    );
    // The browser checks first what it can (the server checks it all);
    // `join` awaits without `async`, which `#[action]` adds.
    let page = app.get("/join").text().to_string();
    assert!(
        page.contains("<input type=\"password\" name=\"password\" required minlength=\"8\">"),
        "{page}"
    );
    let short = app.post_form("/join?/join", &[("name", "ada"), ("password", "short")]);
    assert_eq!(short.status, 422);

    let joined = app.post_form(
        "/join?/join",
        &[("name", "ada"), ("password", "correct horse")],
    );
    assert_eq!(
        (joined.status, joined.header("location")),
        (303, Some("/me"))
    );
    assert!(app.cookie("session").is_some());
    let me = app.get("/me");
    assert!(
        me.status == 200 && me.text().contains("<h1>ada</h1>"),
        "{}",
        me.text()
    );
    assert!(!me.text().contains("<img"));

    // An image is taken; the page shows it, and its address serves it.
    let gif = b"GIF89a\x01\0\x01\0\0\0\0;";
    let r = app.send(upload("/me?/avatar", "avatar", gif));
    assert_eq!(r.status, 200, "{}", r.text());
    assert!(r.text().contains("<img src=\"/avatars/1\""), "{}", r.text());
    let pic = app.get("/avatars/1");
    assert_eq!((pic.status, pic.bytes()), (200, &gif[..]));
    assert_eq!(pic.header("content-type"), Some("image/gif"));
    assert_eq!(pic.header("cache-control"), Some("no-cache"));
    let etag = pic.header("etag").unwrap().to_string();
    app.header("if-none-match", &etag);
    let again = app.get("/avatars/1");
    assert_eq!((again.status, again.bytes().len()), (304, 0));
    assert_eq!(app.get("/avatars/2").status, 404);

    // Not an image, or too big: the page again, the problem by the field.
    let svg = app.send(upload(
        "/me?/avatar",
        "avatar",
        b"<svg onload=\"alert(1)\"/>",
    ));
    assert_eq!(svg.status, 422);
    assert!(
        svg.text()
            .contains("must be a PNG, JPEG, GIF, WebP or AVIF image"),
        "{}",
        svg.text()
    );
    let big = [&gif[..], &[0; 64 * 1024]].concat();
    let big = app.send(upload("/me?/avatar", "avatar", &big));
    assert_eq!(big.status, 422);
    assert!(
        big.text().contains("must be at most 64 KB"),
        "{}",
        big.text()
    );
    // The form is multipart and its input takes images, said nowhere.
    let form = app.get("/me").text().to_string();
    assert!(
        form.contains("<form action=\"?/avatar\" method=\"post\" enctype=\"multipart/form-data\">")
            && form.contains("<input type=\"file\" name=\"avatar\" required accept=\"image/*\">"),
        "{form}"
    );
    let none = app.send(upload("/me?/avatar", "other", gif));
    assert!(none.status == 422 && none.text().contains("choose an image"));
    assert_eq!(app.get("/avatars/1").bytes(), gif, "kept as it was");

    // Out, and in again with the password.
    let left = app.post_form("/me?/leave", &[]);
    assert_eq!(left.header("location"), Some("/"));
    assert!(app.cookie("session").is_none());
    assert_eq!(app.get("/me").status, 303);
    let wrong = app.post_form("/join?/enter", &[("name", "ada"), ("password", "horse")]);
    assert_eq!(wrong.status, 422);
    assert!(
        wrong.text().contains("Wrong name or password"),
        "{}",
        wrong.text()
    );
    // No such name says the same, so a sign-in tells nobody who exists.
    let nobody = app.post_form("/join?/enter", &[("name", "bob"), ("password", "horse")]);
    assert_eq!(nobody.status, 422);
    assert!(nobody.text().contains("Wrong name or password"));
    // A name taken is the one thing sign-up says no to.
    let twice = app.post_form(
        "/join?/join",
        &[("name", "ada"), ("password", "another one")],
    );
    assert_eq!(twice.status, 422);
    assert!(twice.text().contains("Already signed up"));
    let back = app.post_form(
        "/join?/enter",
        &[("name", "ada"), ("password", "correct horse")],
    );
    assert_eq!(back.header("location"), Some("/me"));
    assert_eq!(app.get("/me").status, 200);

    // Signed out everywhere: the session it still sends, as a copy kept by
    // someone else would be, is nobody, and signing in again holds.
    let ended = app.post_form("/me?/everywhere", &[]);
    assert_eq!(ended.header("location"), Some("/"));
    assert!(app.cookie("session").is_some());
    assert_eq!(app.get("/me").status, 303);
    let back = app.post_form(
        "/join?/enter",
        &[("name", "ada"), ("password", "correct horse")],
    );
    assert_eq!(back.header("location"), Some("/me"));
    assert_eq!(app.get("/me").status, 200);
}

#[test]
fn pages_of_rows() {
    let mut app = client::<Site>();
    for text in ["one", "two", "three"] {
        app.post_form("/feed?/add", &[("text", text)]);
    }
    let html = app.get("/feed").text().to_string();
    assert!(
        html.contains("<p>three</p>") && html.contains("<p>two</p>"),
        "{html}"
    );
    assert!(
        !html.contains("<p>one</p>") && !html.contains("Newer"),
        "{html}"
    );
    assert!(html.contains("<a href=\"?page=2\">Older</a>"), "{html}");
    let html = app.get("/feed?page=2").text().to_string();
    assert!(
        html.contains("<p>one</p>") && !html.contains("<p>two</p>"),
        "{html}"
    );
    assert!(
        html.contains("<a href=\"?page=1\">Newer</a>") && !html.contains("Older"),
        "{html}"
    );
}

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

// The tests below write rows under a user of their own, not to `/tasks`,
// whose rows `rest_resources_filter_sort_page_and_hook` counts.

#[test]
fn if_match_compares_strongly() {
    let mut app = client::<Site>();
    let made = app.post_json("/users/41/items", r#"{"name":"Tea"}"#);
    assert_eq!(made.status, 201);
    let at = made.header("location").unwrap().to_string();
    let tag = app.get(&at).header("etag").unwrap().to_string();
    app.header("if-match", &format!("W/{tag}"));
    let weak = app.put_json(&at, r#"{"name":"Weak"}"#);
    assert_eq!(weak.status, 412, "a weak tag promises no bytes");
    app.header("if-match", &tag);
    let strong = app.put_json(&at, r#"{"name":"Strong"}"#);
    assert_eq!(strong.status, 200, "{}", strong.text());

    // A PATCH of many names the type does not have is read once.
    let mut many: String = (0..40_000).map(|i| format!("\"x{i}\":1,")).collect();
    many = format!("{{{many}\"name\":\"Done\"}}");
    let patched = app.patch_json(&at, &many);
    assert_eq!(patched.status, 200);
    assert!(patched.text().contains(r#""name":"Done""#));
}

#[test]
fn one_visitor_never_gets_another_visitors_answer() {
    let mut app = client::<Site>();
    let body = r#"{"name":"Mine"}"#;
    app.header("idempotency-key", "same");
    app.header("cookie", "session=ann");
    let ann = app.post_json("/users/42/items", body);
    app.header("idempotency-key", "same");
    app.header("cookie", "session=bob");
    let bob = app.post_json("/users/42/items", body);
    assert_eq!((ann.status, bob.status), (201, 201));
    assert_eq!(bob.header("idempotent-replayed"), None);
    assert_ne!(ann.header("location"), bob.header("location"));
    app.header("idempotency-key", "same");
    app.header("cookie", "session=ann");
    let again = app.post_json("/users/42/items", body);
    assert_eq!(again.header("idempotent-replayed"), Some("true"));
    assert_eq!(again.text(), ann.text());
}

/// pushState and replaceState need no import: every module gets them from
/// live.js, which sends them to wisp.js, which keeps the history.
#[test]
fn shallow_routing_helpers() {
    let mut app = client::<Site>();
    let page = app.get("/a2/shallow").text().to_string();
    let at = page.find("/_app/c/t").expect("a module");
    let url = &page[at..at + page[at..].find('"').unwrap()];
    let module = app.get(url).text().to_string();
    assert!(
        module.contains("pushState('?tab=2', { tab: 2 })"),
        "{module}"
    );
    assert!(module.contains("pushState, replaceState, "), "{module}");
    let at = page.find("/_app/live.js").expect("the runtime");
    let live = app
        .get(&page[at..at + page[at..].find('"').unwrap()])
        .text()
        .to_string();
    assert!(live.contains("export const pushState") && live.contains("'wisp:push'"));
    let wisp = app.get("/_app/wisp.js").text().to_string();
    assert!(wisp.contains("'wisp:push'") && wisp.contains("'wisp:pop'"));
}

/// Snapshots: wisp.js keeps fields per history entry; a script's
/// `export const snapshot` is handed to extra.js, which its module imports.
#[test]
fn snapshots_are_kept_per_entry() {
    let mut app = client::<Site>();
    let wisp = app.get("/_app/wisp.js").text().to_string();
    assert!(wisp.contains("sessionStorage") && wisp.contains("[autocomplete=off]"));
    let page = app.get("/a2/snap").text().to_string();
    let at = page.find("/_app/c/t").expect("a module");
    let module = app
        .get(&page[at..at + page[at..].find('"').unwrap()])
        .text()
        .to_string();
    assert!(
        module.contains("], snap: snapshot };") && module.contains("/_app/c/extra.js"),
        "{module}"
    );
    // A page without one pays nothing for it.
    let page = app.get("/a2/shallow").text().to_string();
    let at = page.find("/_app/c/t").expect("a module");
    let module = app
        .get(&page[at..at + page[at..].find('"').unwrap()])
        .text()
        .to_string();
    assert!(
        !module.contains("snap:") && !module.contains("extra.js"),
        "{module}"
    );
}

/// Server components: `Plain` has no browser code, so the page names no
/// module for it, though islands sit around and inside it.
#[test]
fn server_components_ship_no_js() {
    let mut app = client::<Site>();
    let page = app.get("/a2/server").text().to_string();
    assert!(page.contains("<p>slotted</p>") && page.contains("<p>direct</p>"));
    let json = &page[page.find("id=\"wisp-live\">").expect("instances")..];
    let map = &json[..json.find("},\"i\"").unwrap()];
    assert_eq!(
        map.matches("/_app/c/t").count(),
        3,
        "Panel, Ping, Tally: {map}"
    );
    // Each Ping waits for itself, inside its Panel island.
    assert_eq!(json.matches(",{},\"v\"]").count(), 3, "{json}");
}

/// `const SSR: bool = false;`: the server sends the page's head, its
/// markup as a template it does not paint, and its data; the browser
/// draws it.
#[test]
fn a_page_without_server_rendering_sends_its_data() {
    let mut app = client::<Site>();
    let page = app.get("/drawn");
    let text = page.text();
    assert_eq!(page.status, 200, "{text}");
    assert!(text.contains("<title>Drawn Tea</title>"), "{text}");
    assert!(text.contains("<template data-w=\""), "{text}");
    // Nothing painted: no copy after the template.
    assert!(!text.contains("<!--[-->"), "{text}");
    let data = &text[text.find("id=\"wisp-live\"").expect("a live page")..];
    assert!(
        data.contains("\"Tea\"") && data.contains("[1,2,3]"),
        "{data}"
    );
}
