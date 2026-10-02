//! Each of the five features the `real` token count is for does what it says.
//! Tables are shared by the tests of this process, so each test signs up
//! its own user, and everything about posts is one test.

use super::App;
use wisp::test::client;
use wisp::{Reply, Request};

/// The signature a PNG starts with, which is all an upload is judged by.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";

fn location(r: &Reply) -> &str {
    r.header("location").unwrap_or("")
}

fn sign_up(app: &mut wisp::test::Client<App>, email: &str) {
    let r = app.post_form("/signup", &[("email", email), ("password", "hunter22")]);
    assert_eq!(
        (r.status, location(&r)),
        (303, "/dashboard"),
        "{}",
        r.text()
    );
}

/// A multipart form with one file, as a browser sends `<input type="file">`.
fn send_avatar(app: &mut wisp::test::Client<App>, kind: &str, bytes: &[u8]) -> Reply {
    let mut body = format!(
        "--XX\r\ncontent-disposition: form-data; name=\"avatar\"; filename=\"a\"\r\ncontent-type: {kind}\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n--XX--\r\n");
    let mut req = Request::new("POST", "/avatar");
    req.header("content-type", "multipart/form-data; boundary=XX");
    req.body = body;
    app.send(req)
}

/// The browser modules a page loads, one after another.
fn modules(app: &mut wisp::test::Client<App>, page: &str) -> String {
    let mut all = String::new();
    let link = "<link rel=\"modulepreload\" href=\"";
    for (at, _) in page.match_indices(link) {
        let url = &page[at + link.len()..];
        all.push_str(app.get(&url[..url.find('"').unwrap()]).text());
    }
    all
}

#[test]
fn auth() {
    let mut app = client::<App>();
    let r = app.get("/dashboard");
    assert_eq!((r.status, location(&r)), (303, "/login"));

    let bad = app.post_form("/signup", &[("email", "ann"), ("password", "short")]);
    assert_eq!(bad.status, 422);
    let page = bad.text();
    assert!(
        page.contains("value=\"ann\"") && page.contains("must be an email"),
        "{page}"
    );
    let short = app.post_form(
        "/signup",
        &[("email", "ann@example.com"), ("password", "short")],
    );
    assert_eq!(short.status, 422);
    assert!(short.text().contains("at least 8"), "{}", short.text());

    sign_up(&mut app, "ann@example.com");
    let dash = app.get("/dashboard");
    assert!(
        dash.text().contains("Signed in as ann@example.com"),
        "{}",
        dash.text()
    );
    // Hashed, not kept as sent.
    let row = crate::db::USERS
        .find(|u| &*u.email == "ann@example.com")
        .unwrap();
    assert!(row.hash.starts_with("$pbkdf2-sha256$"), "{}", row.hash);

    let taken = app.post_form(
        "/signup",
        &[("email", "ann@example.com"), ("password", "whatever1")],
    );
    assert_eq!(taken.status, 422);
    assert!(taken.text().contains("Already signed up"));

    let out = app.post_form("/dashboard?/logout", &[]);
    assert_eq!((out.status, location(&out)), (303, "/login"));
    assert_eq!(app.get("/dashboard").status, 303);

    let wrong = app.post_form(
        "/login",
        &[("email", "ann@example.com"), ("password", "nope")],
    );
    assert_eq!(wrong.status, 422);
    assert!(wrong.text().contains("Wrong email or password"));
    let right = app.post_form(
        "/login",
        &[("email", "ann@example.com"), ("password", "hunter22")],
    );
    assert_eq!((right.status, location(&right)), (303, "/dashboard"));
    assert!(app.get("/dashboard").text().contains("ann@example.com"));
}

#[test]
fn posts_crud_live_and_component() {
    let mut app = client::<App>();
    let mut events = app.get("/posts/events");
    assert_eq!(events.header("content-type"), Some("text/event-stream"));

    // Validation: a 422 that shows the problems and keeps what was typed.
    let bad = app.post_form("/posts/new", &[("title", ""), ("body", "Kept body")]);
    assert_eq!(bad.status, 422);
    let page = bad.text();
    assert!(
        page.contains("<textarea name=\"body\">Kept body</textarea>"),
        "{page}"
    );
    assert!(
        page.contains("<small class=\"problem\">is required</small>"),
        "{page}"
    );
    let long = "x".repeat(101);
    assert_eq!(
        app.post_form("/posts/new", &[("title", &long), ("body", "b")])
            .status,
        422
    );
    assert_eq!(
        app.post_form("/posts/new", &[("title", "t"), ("body", "")])
            .status,
        422
    );

    for n in 1..=12 {
        let r = app.post_form(
            "/posts/new",
            &[("title", &format!("Post {n}")), ("body", "Hi")],
        );
        assert_eq!((r.status, location(&r)), (303, "/posts"));
    }
    // Live: each created post is an event for the open lists.
    let chunk = String::from_utf8(app.next_chunk(&mut events).unwrap()).unwrap();
    assert_eq!(chunk, "data: new\n\n");
    let list = app.get("/posts").text().to_string();
    let scripts = modules(&mut app, &list);
    assert!(
        scripts.contains("listen('/posts/events', invalidate)"),
        "{scripts}"
    );

    // Pages of ten, newest first.
    assert_eq!(list.matches("/edit\">Edit</a>").count(), 10, "{list}");
    assert!(list.contains(">Post 12</button>") && !list.contains(">Post 2</button>"));
    assert!(list.contains("href=\"?page=2\"") && !list.contains("?page=0"));
    let two = app.get("/posts?page=2").text().to_string();
    assert_eq!(two.matches("/edit\">Edit</a>").count(), 2, "{two}");
    assert!(two.contains(">Post 1</button>") && two.contains("href=\"?page=1\""));

    // Edit: the form starts from the post, and a 422 keeps what was sent.
    let edit = app.get("/posts/1/edit").text().to_string();
    assert!(
        edit.contains("value=\"Post 1\"") && edit.contains(">Hi</textarea>"),
        "{edit}"
    );
    let bad = app.post_form("/posts/1/edit", &[("title", ""), ("body", "Changed")]);
    assert_eq!(bad.status, 422);
    assert!(bad.text().contains(">Changed</textarea>") && bad.text().contains("value=\"\""));
    let ok = app.post_form("/posts/1/edit", &[("title", "First"), ("body", "Changed")]);
    assert_eq!((ok.status, location(&ok)), (303, "/posts"));
    assert!(app.get("/posts?page=2").text().contains(">First</button>"));

    // Delete, then a missing post is a 404.
    assert_eq!(app.post_form("/posts?/remove&id=1", &[]).status, 200);
    assert_eq!(app.get("/posts/1/edit").status, 404);
    assert_eq!(
        app.post_form("/posts/1/edit", &[("title", "x"), ("body", "y")])
            .status,
        404
    );
    assert_eq!(app.get("/posts/99/edit").status, 404);

    // The component: a toggle with state of its own, on each post.
    assert!(scripts.contains("open.v = !open.v"), "{scripts}");
    assert_eq!(list.matches("\" hidden>").count(), 10, "{list}");
}

#[test]
fn upload_avatar() {
    let mut app = client::<App>();
    assert_eq!(app.get("/avatar").status, 303);
    assert_eq!(send_avatar(&mut app, "image/png", b"png").status, 303);
    sign_up(&mut app, "bob@example.com");
    assert!(!app.get("/dashboard").text().contains("<img"));

    let text = send_avatar(&mut app, "text/plain", b"hello");
    assert_eq!(text.status, 422);
    assert!(
        text.text()
            .contains("must be a PNG, JPEG, GIF, WebP or AVIF image"),
        "{}",
        text.text()
    );
    let mut png = PNG.to_vec();
    png.resize(1024 * 1024 + 1, 0);
    let big = send_avatar(&mut app, "image/png", &png);
    assert_eq!(big.status, 422);
    assert!(
        big.text().contains("must be at most 1 MB"),
        "{}",
        big.text()
    );

    let ok = send_avatar(&mut app, "image/png", PNG);
    assert_eq!(
        (ok.status, location(&ok)),
        (303, "/dashboard"),
        "{}",
        ok.text()
    );
    let dash = app.get("/dashboard").text().to_string();
    let at = dash.find("<img src=\"").expect("the avatar") + 10;
    let src = &dash[at..at + dash[at..].find('"').unwrap()];
    let img = app.get(src);
    assert_eq!(img.header("content-type"), Some("image/png"));
    assert_eq!(img.bytes(), PNG);
    assert_eq!(app.get("/avatars/9999").status, 404);
}

#[test]
fn component_on_the_dashboard() {
    let mut app = client::<App>();
    sign_up(&mut app, "cy@example.com");
    let dash = app.get("/dashboard").text().to_string();
    assert!(dash.contains(">Account</button>"), "{dash}");
    assert!(
        dash.contains("<p>Signed up with cy@example.com</p>"),
        "{dash}"
    );
    assert!(modules(&mut app, &dash).contains("open.v = !open.v"));
}
