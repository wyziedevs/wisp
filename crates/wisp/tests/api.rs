//! What an app does through `Cx` and `Response`, answered in process by
//! `wisp::test::client` on the hand-written app in `common/`: cookies,
//! signed cookies, auth, CORS, errors, redirects, files, streams, request
//! ids, limits and the dev endpoint.

mod common;

use common::{DATA, Lab, ROOT, TEMPLATE};
use std::net::SocketAddr;
use std::path::Path;
use wisp::{Reply, Request};

/// A test client for the app, once the files dev builds serve exist (and one
/// they must not serve): the server lists `static/` as it gets ready, so they
/// are written first, once for all the tests of a process.
pub fn client() -> wisp::test::Client<Lab> {
    static WRITTEN: std::sync::Once = std::sync::Once::new();
    WRITTEN.call_once(write_files);
    wisp::test::client::<Lab>()
}

fn write_files() {
    let root = Path::new(ROOT);
    let write = |path: &str, text: &str| {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    };
    write("static/ok.txt", "ok");
    write("static/a b.txt", "spaced");
    write("static/café.txt", "accented");
    write("static/asset.txt", "asset");
    write("static/sub/deep.json", "{}");
    write("secret.txt", "secret");
}

fn request(method: &str, target: &str, headers: &[(&str, &str)], body: &[u8]) -> Request {
    let mut req = Request::new(method, target);
    for (name, value) in headers {
        req.header(name, value);
    }
    req.body = body.to_vec();
    req
}

fn get(target: &str, headers: &[(&str, &str)]) -> Reply {
    client().send(request("GET", target, headers, b""))
}

fn header<'a>(reply: &'a Reply, name: &str) -> &'a str {
    reply.header(name).unwrap_or("")
}

#[test]
fn pages_and_the_shell() {
    let mut app = client();
    let page = app.get("/hello");
    assert_eq!(page.status, 200);
    assert_eq!(header(&page, "content-type"), "text/html; charset=utf-8");
    assert!(
        page.text().starts_with("<!doctype html><html><head>"),
        "{}",
        page.text()
    );
    assert!(page.text().contains("<title>lab</title>"));
    assert!(page.text().ends_with("<h1>hello</h1></body></html>"));

    // HEAD has the headers of a GET, its length included, and no body.
    let head = app.send(request("HEAD", "/hello", &[], b""));
    assert_eq!(head.status, 200);
    assert_eq!(head.text(), "");
    assert_eq!(
        header(&head, "content-length"),
        page.text().len().to_string()
    );

    let missing = app.get("/nowhere");
    assert_eq!(missing.status, 404);
    assert!(missing.text().contains("[404:"), "{}", missing.text());
    assert_eq!(app.get("/stray").status, 404);

    let status = app.get("/status?n=201");
    assert_eq!(
        (status.status, status.text().contains("status")),
        (201, true)
    );
    // A status no response can have is a bug of the app's: a 500, not a broken reply.
    assert_eq!(app.get("/status?n=1000").status, 500);
}

#[test]
fn queries_params_and_bodies() {
    let mut app = client();
    let q = app.get("/q?n=7&s=a%20b+c&s=second");
    assert_eq!(q.text(), "n=7&s=a%20b+c&s=second|7|a b c");
    // Not a number: the default.
    assert_eq!(app.get("/q?n=x").text(), "n=x|5|");
    assert_eq!(app.get("/q").text(), "|5|");

    assert_eq!(app.get("/item/a%20b").text(), "item a b 0");
    let ok = app.send(request("POST", "/item/x", &[], &[b'a'; 16]));
    assert_eq!(ok.text(), "item x 16");
    // The route's own limit, from `body_limit`, before the handler runs.
    let big = app.send(request("POST", "/item/x", &[], &[b'a'; 17]));
    assert_eq!(big.status, 413);

    let echo = app.send(request("POST", "/echo", &[], b"hello"));
    assert_eq!(echo.text(), "5:hello");
    // The app-wide limit is 1 MB unless `WISP_BODY_LIMIT` says otherwise.
    let one_mb = 1 << 20;
    assert_eq!(
        app.send(request("POST", "/echo", &[], &vec![0; one_mb]))
            .status,
        200
    );
    assert_eq!(
        app.send(request("POST", "/echo", &[], &vec![0; one_mb + 1]))
            .status,
        413
    );

    assert_eq!(app.get("/host").text(), "None");
    assert_eq!(
        get("/host", &[("host", "example.com:80")]).text(),
        "Some(\"example.com:80\")"
    );
}

#[test]
fn requests_from_other_hosts_are_checked_like_the_wire_ones() {
    let mut app = client();
    let mut send = |req: Request| app.send(req).status;
    assert_eq!(send(request("G ET", "/hello", &[], b"")), 400);
    assert_eq!(send(request("", "/hello", &[], b"")), 400);
    assert_eq!(send(request("GET1", "/hello", &[], b"")), 400);
    assert_eq!(send(request("GET", "/a b", &[], b"")), 400);
    assert_eq!(send(request("GET", "/a\r\nx: y", &[], b"")), 400);
    assert_eq!(send(request("GET", "/a\u{7f}", &[], b"")), 400);
    assert_eq!(send(request("GET", "/hello", &[("x:y", "1")], b"")), 400);
    assert_eq!(send(request("GET", "/hello", &[("", "1")], b"")), 400);
    assert_eq!(
        send(request("GET", "/hello", &[("x-a", "1\r\nx-b: 2")], b"")),
        400
    );
    assert_eq!(send(request("GET", "/hello", &[("x-a", "1\0")], b"")), 400);
    // The framing is the host's: what the caller says about it is replaced.
    let lying = request(
        "POST",
        "/echo",
        &[("content-length", "99"), ("transfer-encoding", "chunked")],
        b"abc",
    );
    assert_eq!(app.send(lying).text(), "3:abc");

    let many: Vec<(String, String)> = (0..100).map(|i| (format!("x-{i}"), "1".into())).collect();
    let mut req = Request::new("GET", "/hello");
    req.headers = many;
    assert_eq!(app.send(req).status, 431);
    let mut req = Request::new("GET", "/hello");
    req.header("x-big", &"a".repeat(20_000));
    assert_eq!(app.send(req).status, 431);
    let mut req = Request::new("GET", "/hello");
    req.header("x-fine", &"a".repeat(8_000));
    assert_eq!(app.send(req).status, 200);
}

#[test]
fn cookies_are_written_as_asked() {
    let cookie = |query: &str, headers: &[(&str, &str)]| {
        let reply = get(query, headers);
        assert_eq!(reply.status, 200, "{query}");
        header(&reply, "set-cookie").to_string()
    };
    assert_eq!(
        cookie("/set-cookie?v=1", &[]),
        "a=1; Path=/; Max-Age=60; HttpOnly; SameSite=Lax"
    );
    // A session cookie, readable by scripts, on another path and a domain.
    assert_eq!(
        cookie("/set-cookie?v=1&o=xrpd", &[]),
        "a=1; Path=/p; Domain=example.com; SameSite=Lax"
    );
    assert_eq!(
        cookie("/set-cookie?v=1&o=s", &[]),
        "a=1; Path=/; Max-Age=60; HttpOnly; SameSite=Strict"
    );
    // `SameSite=None` is only kept over HTTPS, so it is always `Secure`.
    assert_eq!(
        cookie("/set-cookie?v=1&o=n", &[]),
        "a=1; Path=/; Max-Age=60; HttpOnly; Secure; SameSite=None"
    );
    // Behind a proxy that speaks HTTPS.
    assert_eq!(
        cookie("/set-cookie?v=1", &[("x-forwarded-proto", "https, http")]),
        "a=1; Path=/; Max-Age=60; HttpOnly; Secure; SameSite=Lax"
    );
    assert!(
        !cookie("/set-cookie?v=1", &[("x-forwarded-proto", "http")]).contains("Secure"),
        "plain HTTP is not secure"
    );
    // An empty value deletes it.
    assert_eq!(
        cookie("/set-cookie?v=", &[]),
        "a=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax"
    );
    let signed = cookie("/set-cookie?v=1&o=g", &[]);
    let (value, mac) = signed
        .split(';')
        .next()
        .and_then(|c| c.strip_prefix("a="))
        .and_then(|v| v.split_once('.'))
        .unwrap();
    assert_eq!(value, "1");
    assert_eq!(mac.len(), 43, "an HMAC-SHA256 in base64url, unpadded");
}

#[test]
fn cookies_a_value_cannot_break_out_of() {
    // What would end the cookie, or the header, is a bug of the app's: a 500.
    for bad in [
        "a+b", "a%3Bb", "a%2Cb", "a%22b", "a%5Cb", "a%0D%0Ax", "%C3%A9",
    ] {
        let reply = get(&format!("/set-cookie?v={bad}"), &[]);
        assert_eq!(reply.status, 500, "{bad}");
        assert_eq!(reply.header("set-cookie"), None, "{bad}");
    }
}

#[test]
fn cookies_are_read_back() {
    let read = |cookie: &str| get("/get-cookie", &[("cookie", cookie)]).text().to_string();
    assert_eq!(read("a=12"), "Some(\"12\")|12|None|9");
    assert_eq!(read("a=junk"), "Some(\"junk\")|0|None|9");
    assert_eq!(read("b=1; a=\"quoted\" ; c=2"), "Some(\"quoted\")|0|None|9");
    assert_eq!(read("x=1"), "None|0|None|9");
    assert_eq!(read("=;;=;"), "None|0|None|9");
    assert_eq!(get("/get-cookie", &[]).text(), "None|0|None|9");

    // What this request stored is what it reads back; deleting shows at once.
    let deleted = get("/delete-cookie", &[("cookie", "a=1")]);
    assert_eq!(deleted.text(), "None");
    assert!(header(&deleted, "set-cookie").contains("Max-Age=0"));
}

#[test]
fn signed_cookies_cannot_be_forged() {
    let mut app = client();
    assert_eq!(app.get("/who").text(), "nobody");
    app.get("/sign?v=ada");
    let stored = app.cookie("user").unwrap().to_string();
    let (value, mac) = stored.split_once('.').unwrap();
    assert_eq!(value, "ada");
    assert_eq!(app.get("/who").text(), "ada");

    let who = |cookie: String| get("/who", &[("cookie", &cookie)]).text().to_string();
    assert_eq!(who(format!("user={stored}")), "ada");
    // Another value under the same signature.
    assert_eq!(who(format!("user=eve.{mac}")), "nobody");
    // Nothing signed, or nothing left of the signature.
    assert_eq!(who("user=ada".into()), "nobody");
    assert_eq!(who("user=ada.".into()), "nobody");
    assert_eq!(who("user=.".into()), "nobody");
    assert_eq!(who(format!("user=ada.{}", &mac[1..])), "nobody");
    assert_eq!(who(format!("user=ada.{mac}x")), "nobody");
    assert_eq!(
        who(format!("user=ada.{}", mac.to_ascii_uppercase())),
        "nobody"
    );
    assert_eq!(who(format!("user={stored}.{mac}")), "nobody");
    // The signature holds the name: the same value and signature as another cookie's.
    let other = get("/set-cookie?v=ada&o=g", &[]);
    let a = header(&other, "set-cookie")
        .split(';')
        .next()
        .unwrap()
        .replace("a=", "user=");
    assert_ne!(who(a), "ada", "a signature moved to another cookie's name");

    // Signed cookies read through `signed_cookie_or` too.
    let signed = get("/set-cookie?v=41&o=g", &[]);
    let a = header(&signed, "set-cookie")
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let seen = get("/get-cookie", &[("cookie", &a)]);
    assert_eq!(seen.text().rsplit('|').next(), Some("41"));
    let tampered = get("/get-cookie", &[("cookie", &a.replace("a=41", "a=42"))]);
    assert_eq!(tampered.text().rsplit('|').next(), Some("9"));
}

#[test]
fn a_message_is_flashed_once() {
    let mut app = client();
    let flash = app.get("/flash?m=Saved%20it");
    assert_eq!(
        (flash.status, header(&flash, "location")),
        (303, "/flashed")
    );
    // The next page reads it; a second read in the same request finds nothing.
    assert_eq!(app.get("/flashed").text(), "Some(\"Saved it\")|None");
    // And it is gone for the page after.
    assert_eq!(app.get("/flashed").text(), "None|None");
    // Any text is safe in one.
    app.get("/flash?m=a%3Bb%2C%22c%22%20%E2%98%85%25");
    assert_eq!(
        app.get("/flashed").text(),
        "Some(\"a;b,\\\"c\\\" ★%\")|None"
    );
}

#[test]
fn auth_from_headers() {
    let bearer = |auth: Option<&str>| {
        let headers: Vec<(&str, &str)> = auth.map(|a| ("authorization", a)).into_iter().collect();
        get("/bearer", &headers)
    };
    let ok = bearer(Some("Bearer wisp"));
    assert_eq!((ok.status, ok.text()), (200, "wisp"));
    assert_eq!(bearer(Some("bearer wisp")).status, 200);
    let denied = bearer(None);
    assert_eq!(denied.status, 401);
    assert_eq!(header(&denied, "www-authenticate"), "Bearer");
    for wrong in [
        "Bearer wisp2",
        "Bearer wis",
        "Bearer ",
        "Bearer",
        "Basic wisp",
        "wisp",
    ] {
        assert_eq!(bearer(Some(wrong)).status, 401, "{wrong}");
    }
    // A key that is not set matches nothing, not even an empty token.
    let unset = get("/unset-key", &[("authorization", "Bearer ")]);
    assert_eq!(unset.status, 401);
    assert_eq!(
        get("/unset-key", &[("authorization", "Bearer x")]).status,
        401
    );

    let mut app = client();
    app.bearer("wisp");
    assert_eq!(app.get("/bearer").status, 200);
    // A request's own header wins over the client's.
    let own = app.send(request(
        "GET",
        "/bearer",
        &[("authorization", "Bearer no")],
        b"",
    ));
    assert_eq!(own.status, 401);

    let basic = |value: &str| {
        get("/basic", &[("authorization", value)])
            .text()
            .to_string()
    };
    assert_eq!(
        basic("Basic YWRhOnNlY3JldA=="),
        "Some((\"ada\", \"secret\"))"
    );
    assert_eq!(basic("basic YWRhOnNlY3JldA"), "Some((\"ada\", \"secret\"))");
    // Only the first colon divides them.
    assert_eq!(basic("Basic YWRhOnB3Ong="), "Some((\"ada\", \"pw:x\"))");
    assert_eq!(basic("Basic YWRhOg=="), "Some((\"ada\", \"\"))");
    for wrong in [
        "Basic bm9jb2xvbg==",
        "Basic !!!",
        "Basic",
        "Bearer YWRhOnNlY3JldA==",
        "",
    ] {
        assert_eq!(basic(wrong), "None", "{wrong}");
    }
    assert_eq!(get("/basic", &[]).text(), "None");
}

#[test]
fn which_methods_write() {
    let writes = |method: &str| {
        client()
            .send(request(method, "/writes", &[], b""))
            .text()
            .to_string()
    };
    for (method, expected) in [
        ("GET", "false"),
        ("HEAD", ""),
        ("OPTIONS", "false"),
        ("POST", "true"),
        ("PUT", "true"),
        ("PATCH", "true"),
        ("DELETE", "true"),
    ] {
        assert_eq!(writes(method), expected, "{method}");
    }
}

#[test]
fn cors_answers_the_sites_it_allows() {
    let cors = |origins: &str, method: &str, headers: &[(&str, &str)]| {
        client().send(request(method, &format!("/cors?o={origins}"), headers, b""))
    };
    // No `Origin`: not a browser call across sites; nothing to add.
    let none = cors("*", "GET", &[]);
    assert_eq!(none.text(), "body");
    assert_eq!(none.header("vary"), None);

    let any = cors("*", "GET", &[("origin", "https://app.example.com")]);
    assert_eq!(header(&any, "access-control-allow-origin"), "*");
    assert_eq!(any.header("access-control-allow-credentials"), None);
    assert_eq!(header(&any, "vary"), "origin");
    assert_eq!(any.text(), "body");

    // A list: the caller's own origin is echoed, and may send cookies.
    let listed = cors(
        "https://x.example%20https://app.example.com/",
        "GET",
        &[("origin", "HTTPS://APP.example.com")],
    );
    assert_eq!(
        header(&listed, "access-control-allow-origin"),
        "HTTPS://APP.example.com"
    );
    assert_eq!(header(&listed, "access-control-allow-credentials"), "true");
    let commas = cors(
        "https://a.example,https://b.example",
        "GET",
        &[("origin", "https://b.example")],
    );
    assert_eq!(
        header(&commas, "access-control-allow-origin"),
        "https://b.example"
    );

    // A site that is not listed gets no permission, so its browser hides the answer.
    for origin in [
        "https://evil.example",
        "https://app.example.com.evil.example",
        "null",
    ] {
        let denied = cors("https://app.example.com", "GET", &[("origin", origin)]);
        assert_eq!(
            denied.header("access-control-allow-origin"),
            None,
            "{origin}"
        );
        assert_eq!(denied.header("access-control-allow-credentials"), None);
        assert_eq!(header(&denied, "vary"), "origin");
    }

    // The preflight is answered without reaching the route.
    let pre = cors(
        "*",
        "OPTIONS",
        &[
            ("origin", "https://a.example"),
            ("access-control-request-method", "PUT"),
            (
                "access-control-request-headers",
                "authorization, content-type",
            ),
        ],
    );
    assert_eq!(pre.status, 204);
    assert_eq!(header(&pre, "access-control-allow-methods"), "PUT");
    assert_eq!(
        header(&pre, "access-control-allow-headers"),
        "authorization, content-type"
    );
    assert_eq!(header(&pre, "access-control-max-age"), "86400");
    assert_eq!(pre.text(), "");
    // An OPTIONS that asks for no method is not a preflight.
    let plain = cors("*", "OPTIONS", &[("origin", "https://a.example")]);
    assert_eq!(plain.text(), "body");
    // A preflight from a site that is not allowed is not answered as one.
    let refused = cors(
        "https://app.example.com",
        "OPTIONS",
        &[
            ("origin", "https://evil.example"),
            ("access-control-request-method", "PUT"),
        ],
    );
    assert_eq!(refused.text(), "body");
}

#[test]
fn values_travel_with_the_request() {
    let mut app = client();
    assert_eq!(app.get("/state").text(), "Some(8)|Some(8)|None|lab");
    let failed = app.get("/fail");
    assert_eq!(
        (failed.status, failed.text().contains("failed 3")),
        (422, true)
    );
}

#[test]
fn errors_by_status_and_format() {
    let mut app = client();
    let teapot = app.get("/err?k=teapot");
    assert_eq!(teapot.status, 418);
    assert_eq!(header(&teapot, "content-type"), "text/html; charset=utf-8");
    assert!(
        teapot.text().contains("[418:short and stout]"),
        "{}",
        teapot.text()
    );

    // The same error as JSON: under /api, when JSON was sent, or when it is all the client accepts.
    let json = |reply: &Reply| {
        assert_eq!(header(reply, "content-type"), "application/json");
        reply.json::<wisp::Value>()
    };
    let says = |v: &wisp::Value, status: i64, error: &str| {
        assert_eq!(v.get("status").and_then(wisp::Value::as_i64), Some(status));
        assert_eq!(v.get("error").and_then(wisp::Value::as_str), Some(error));
    };
    let accept = get("/err?k=teapot", &[("accept", "application/json")]);
    says(&json(&accept), 418, "short and stout");
    let sent = app.send(request(
        "POST",
        "/err?k=teapot",
        &[("content-type", "application/json")],
        b"{}",
    ));
    says(&json(&sent), 418, "short and stout");
    let api = get("/api/x", &[]);
    assert_eq!(api.status, 404);
    assert_eq!(
        json(&api).get("status").and_then(wisp::Value::as_i64),
        Some(404)
    );
    assert_eq!(
        header(&get("/api", &[]), "content-type"),
        "application/json"
    );
    assert_eq!(
        header(&get("/apiary", &[]), "content-type"),
        "text/html; charset=utf-8",
        "only /api and what is below it"
    );
    // A browser that also takes HTML gets the page.
    let browser = get(
        "/err?k=teapot",
        &[("accept", "text/html,application/json;q=0.9")],
    );
    assert_eq!(header(&browser, "content-type"), "text/html; charset=utf-8");

    let invalid = get("/err?k=invalid", &[("accept", "application/json")]);
    assert_eq!(invalid.status, 422);
    let body = json(&invalid);
    says(&body, 422, "a: is bad; b: is worse");
    let errors = body.get("errors").unwrap();
    assert_eq!(
        errors.get("a").and_then(wisp::Value::as_str),
        Some("is bad")
    );
    assert_eq!(
        errors.get("b").and_then(wisp::Value::as_str),
        Some("is worse")
    );

    let retry = app.get("/err?k=retry");
    assert_eq!((retry.status, header(&retry, "retry-after")), (429, "5"));

    let moved = app.get("/err?k=moved");
    assert_eq!((moved.status, header(&moved, "location")), (308, "/new"));
    assert_eq!(moved.text(), "");

    // Errors that say no more than their status get a sentence.
    let empty = app.get("/err?k=empty");
    assert_eq!(empty.status, 503);
    assert!(
        empty.text().contains("cannot take requests right now"),
        "{}",
        empty.text()
    );
    let none = app.get("/err?k=or404");
    assert_eq!(none.status, 404);
    assert!(none.text().contains("There is nothing at this address."));
    let bad = app.get("/err?k=or400");
    assert_eq!(bad.status, 400);
    assert!(
        bad.text().contains("invalid digit found in string"),
        "{}",
        bad.text()
    );

    // A failure inside is a 500 whose details only dev builds show; the connection's next request works.
    for kind in ["io", "panic"] {
        let failed = app.get(&format!("/err?k={kind}"));
        assert_eq!(failed.status, 500, "{kind}");
        let told = ["disk on fire", "boom"]
            .iter()
            .any(|s| failed.text().contains(s));
        assert_eq!(told, cfg!(debug_assertions), "{kind}: {}", failed.text());
    }
    assert_eq!(app.get("/err?k=none").status, 200);
}

#[test]
fn the_built_in_error_page_escapes_what_it_shows() {
    let page = get("/default-error?a=<script>alert(1)</script>", &[]);
    assert_eq!(page.status, 404);
    let html = page.text();
    assert!(html.contains("class=\"wisp-error\""), "{html}");
    assert!(html.contains("<title>Not Found</title>"), "{html}");
    assert!(html.contains("gone"));
    assert!(!html.contains("<script>alert"), "{html}");
    assert!(
        !html.contains("href="),
        "the page is the status alone: {html}"
    );
}

#[test]
fn a_redirect_reaches_wisp_js_as_a_header() {
    let plain = get("/redirect-to?to=/next%3Fa=1", &[]);
    assert_eq!(
        (plain.status, header(&plain, "location")),
        (303, "/next?a=1")
    );
    // `wisp.js` goes there itself: fetch would follow with the post's own headers.
    let js = get("/redirect-to?to=/next", &[("x-wisp", "1")]);
    assert_eq!(js.status, 200);
    assert_eq!(js.header("location"), None);
    assert_eq!(header(&js, "x-wisp-location"), "/next");
}

#[test]
fn nothing_splits_a_response() {
    // A value from the request that reaches a header is a bug of the app's:
    // a 500, and never a second header or a second response.
    for target in [
        "/header?v=a%0D%0Aset-cookie:%20x=1",
        "/header?v=a%0Ax:%20y",
        "/header?v=a%00b",
        "/error-header?v=a%0D%0Aset-cookie:%20x=1",
        "/redirect-to?to=/x%0D%0Aset-cookie:%20x=1",
        "/redirect-to?to=/x%0Aset-cookie:%20x=1",
    ] {
        let reply = get(target, &[]);
        assert_eq!(reply.status, 500, "{target}");
        assert_eq!(reply.header("set-cookie"), None, "{target}");
        assert_eq!(reply.header("x-echo"), None, "{target}");
        assert_eq!(reply.header("location"), None, "{target}");
    }
    // Ordinary text passes, spaces and all.
    let fine = get("/header?v=a%20b%3B%20c", &[]);
    assert_eq!(header(&fine, "x-echo"), "a b; c");

    let builders = std::panic::catch_unwind(|| {
        let _ = wisp::Response::text("x").with_header("x", "a\r\nb");
    });
    assert!(builders.is_err());
    assert!(std::panic::catch_unwind(|| wisp::Error::redirect(303, "/a\r\nb")).is_err());
    assert!(
        std::panic::catch_unwind(|| wisp::Error::new(400, "x").with_header("x", "a\nb")).is_err()
    );
}

#[test]
fn responses_of_every_kind() {
    let mut app = client();
    let json = app.get("/json");
    assert_eq!(header(&json, "content-type"), "application/json");
    assert_eq!(json.text(), r#"["a\"b","c"]"#);
    let created = app.get("/json?k=created");
    assert_eq!((created.status, created.text()), (201, "[1,2]"));

    let html = app.get("/resp?k=html");
    assert_eq!(header(&html, "content-type"), "text/html; charset=utf-8");
    let redirect = app.get("/resp?k=redirect");
    assert_eq!(
        (redirect.status, header(&redirect, "location")),
        (303, "/there")
    );
    let empty = app.get("/resp?k=empty");
    assert_eq!(empty.status, 204);
    assert_eq!(empty.header("content-type"), None);
    assert_eq!(empty.header("content-length"), None);
    assert_eq!(header(&empty, "x-a"), "1");
    assert_eq!(empty.text(), "");
    let teapot = app.get("/resp?k=status");
    assert_eq!((teapot.status, teapot.text()), (418, "teapot"));

    let csv = app.get("/resp?k=download&n=report.CSV");
    assert_eq!(header(&csv, "content-type"), "text/csv");
    assert_eq!(
        header(&csv, "content-disposition"),
        "attachment; filename=\"report.CSV\""
    );
    // A name cannot end the quotes or the line.
    let odd = app.get("/resp?k=download&n=my%20%22x%22%5C%0A.csv");
    assert_eq!(odd.status, 200);
    assert_eq!(
        header(&odd, "content-disposition"),
        "attachment; filename=\"my _x___.csv\""
    );
    let unknown = app.get("/resp?k=download&n=data.zzz");
    assert_eq!(header(&unknown, "content-type"), "application/octet-stream");
}

#[test]
fn files_are_read_from_inside_their_folder() {
    let mut app = client();
    let file = |app: &mut wisp::test::Client<Lab>, name: &str| app.get(&format!("/file?n={name}"));
    let hello = file(&mut app, "hello.txt");
    assert_eq!(hello.status, 200);
    assert_eq!(hello.text(), "hello file");
    assert_eq!(header(&hello, "content-type"), "text/plain; charset=utf-8");
    let json = file(&mut app, "sub%2Fx.json");
    assert_eq!(
        (json.text(), header(&json, "content-type")),
        ("{\"a\":1}", "application/json")
    );

    let outside = std::fs::canonicalize(DATA)
        .unwrap()
        .parent()
        .unwrap()
        .join("api.rs");
    assert!(outside.is_file(), "the file the attacks aim at exists");
    for name in [
        "missing.txt",
        "..%2Fapi.rs",
        "sub%2F..%2F..%2Fapi.rs",
        "sub%2F..%2Fhello.txt",
        "..",
        ".",
        "%2Fetc%2Fpasswd",
        "%2F",
        "sub",
        "sub%2F",
        "%5C..%5Capi.rs",
        "..%5Capi.rs",
        "C%3A%2Fwindows%2Fwin.ini",
        "C%3Aapi.rs",
        "hello.txt%00.png",
        "hello.txt%2F",
        "%2Ehello.txt%2F..",
        "sub%2F%2Fx.json",
        "%2F%2Fhello.txt",
    ] {
        let reply = file(&mut app, name);
        assert_eq!(reply.status, 404, "{name}");
        assert!(
            !reply.text().contains("mod common"),
            "{name} read a file outside"
        );
    }
    assert_eq!(app.get("/file").status, 404, "no name at all");
}

#[test]
fn static_files_are_served_from_static_only() {
    let mut app = client();
    let ok = app.get("/ok.txt");
    assert_eq!((ok.status, ok.text()), (200, "ok"));
    assert_eq!(header(&ok, "content-type"), "text/plain; charset=utf-8");
    let deep = app.get("/sub/deep.json");
    assert_eq!(
        (deep.status, header(&deep, "content-type")),
        (200, "application/json")
    );
    let asset = app.get("/asset.txt");
    assert_eq!((asset.status, asset.text()), (200, "asset"));
    // Names as browsers send them: spaces and other characters percent-encoded.
    assert_eq!(app.get("/a%20b.txt").text(), "spaced");
    assert_eq!(app.get("/caf%C3%A9.txt").text(), "accented");
    // Names are exact: a file system that ignores case does not make `/OK.TXT` this file.
    for other in [
        "/OK.txt",
        "/Ok.TXT",
        "/caf%C3%89.txt",
        "/ok.txt.",
        "/ok.txt%20",
    ] {
        assert_eq!(app.get(other).status, 404, "{other}");
    }

    // HEAD is a GET without the body.
    let head = app.send(request("HEAD", "/ok.txt", &[], b""));
    assert_eq!(
        (head.status, head.text(), header(&head, "content-length")),
        (200, "", "2")
    );
    // Only GET and HEAD read files.
    assert_ne!(app.send(request("POST", "/ok.txt", &[], b"")).text(), "ok");

    assert!(std::path::Path::new(ROOT).join("secret.txt").is_file());
    for path in [
        "/../secret.txt",
        "/%2e%2e/secret.txt",
        "/..%2fsecret.txt",
        "/%2e%2e%5csecret.txt",
        "/static/../secret.txt",
        "/sub/../../secret.txt",
        "/sub/%2e%2e/%2e%2e/secret.txt",
        "//secret.txt",
        "/./ok.txt/../../secret.txt",
        "/sub%00/deep.json",
        "/sub",
        "/sub/",
        "/C:/secret.txt",
        "/%5Csecret.txt",
    ] {
        let reply = app.get(path);
        assert!(
            !reply.text().contains("secret") || reply.text().contains('['),
            "{path}: {}",
            reply.text()
        );
        assert_ne!(reply.text(), "secret", "{path}");
        assert_ne!(reply.status, 200, "{path}");
    }
}

#[test]
fn the_browser_script_is_cached_by_its_version() {
    let mut app = client();
    let script = app.get("/_app/wisp.js");
    assert_eq!(script.status, 200);
    assert_eq!(
        header(&script, "content-type"),
        "text/javascript; charset=utf-8"
    );
    assert!(script.text().len() > 1000);
    let etag = header(&script, "etag").to_string();
    assert!(etag.starts_with('"') && etag.ends_with('"'), "{etag}");
    assert_eq!(
        header(&script, "cache-control"),
        "public, max-age=0, must-revalidate"
    );

    let same = app.send(request(
        "GET",
        "/_app/wisp.js",
        &[("if-none-match", &etag)],
        b"",
    ));
    assert_eq!(same.status, 304);
    assert_eq!(same.text(), "");
    assert_eq!(
        same.header("content-type"),
        None,
        "a 304 describes what it did not send"
    );
    assert_eq!(same.header("content-length"), None);
    let other = app.send(request(
        "GET",
        "/_app/wisp.js",
        &[("if-none-match", "\"other\"")],
        b"",
    ));
    assert_eq!(other.status, 200);

    let versioned = app.get("/_app/wisp.js?v=abc");
    assert_eq!(
        header(&versioned, "cache-control"),
        "public, max-age=31536000, immutable"
    );
    assert_eq!(app.get("/_app/live.js").status, 200);
    assert_eq!(app.get("/_app/c/none.js").status, 404);
}

#[test]
fn slashes_never_leave_the_site() {
    let mut app = client();
    for (from, to) in [
        ("/hello/", "/hello"),
        ("/hello/?a=1&b=2", "/hello?a=1&b=2"),
        ("//evil.example/", "/evil.example"),
        ("///evil.example///", "/evil.example"),
        ("/%2Fevil.example/", "/%2Fevil.example"),
    ] {
        let reply = app.get(from);
        assert_eq!(
            (reply.status, header(&reply, "location")),
            (308, to),
            "{from}"
        );
    }
    // A backslash is a slash to some browsers.
    let back = app.send(request("GET", "/\\evil.example/", &[], b""));
    assert_eq!(header(&back, "location"), "/evil.example");
}

#[test]
fn a_rate_limit_counts_by_key_and_refills() {
    let mut app = client();
    let from = |ip: [u8; 4]| {
        let mut req = Request::new("GET", "/limit");
        req.peer = SocketAddr::from((ip, 1));
        req
    };
    assert_eq!(app.send(from([10, 0, 0, 1])).status, 200);
    assert_eq!(app.send(from([10, 0, 0, 1])).status, 200);
    let over = app.send(from([10, 0, 0, 1]));
    assert_eq!(over.status, 429);
    let wait: u64 = header(&over, "retry-after").parse().unwrap();
    assert!((1..=60).contains(&wait), "{wait}");
    // Another client has its own.
    assert_eq!(app.send(from([10, 0, 0, 2])).status, 200);
    // Not moved by a header any client can send.
    let mut spoof = from([10, 0, 0, 1]);
    spoof.header("x-forwarded-for", "10.9.9.9");
    assert_eq!(app.send(spoof).status, 429);
}

#[test]
fn a_bucket_refills_over_its_window() {
    use std::time::Duration;
    // Used up for good, as far as a test can tell: the window is an hour.
    let limit = wisp::RateLimit::per_hour(2);
    assert!(limit.check("k").is_ok());
    assert!(limit.check("k").is_ok());
    let err = limit.check("k").unwrap_err();
    assert_eq!(err.status(), 429);
    assert!(limit.check("other").is_ok());
    // A short window gives the share back once it has passed.
    let quick = wisp::RateLimit::new(1, Duration::from_millis(40));
    assert!(quick.check("k").is_ok());
    std::thread::sleep(Duration::from_millis(80));
    assert!(quick.check("k").is_ok(), "a request's share came back");
    assert!(wisp::RateLimit::per_second(1).check(1).is_ok());
    assert!(std::panic::catch_unwind(|| wisp::RateLimit::new(0, Duration::from_secs(1))).is_err());
}

#[test]
fn the_client_and_the_peer() {
    let mut app = client();
    assert_eq!(app.get("/peer").text(), "127.0.0.1:0|127.0.0.1");
    let mut req = Request::new("GET", "/peer");
    req.peer = "10.1.2.3:5555".parse().unwrap();
    // `X-Forwarded-For` says nothing unless `WISP_CLIENT_IP_HEADER` names it.
    req.header("x-forwarded-for", "6.6.6.6");
    assert_eq!(app.send(req).text(), "10.1.2.3:5555|10.1.2.3");
}

#[test]
fn a_request_id_is_echoed_or_made() {
    let mut app = client();
    assert_eq!(
        app.get("/hello").header("x-request-id"),
        None,
        "nobody asked"
    );
    let echoed = get("/id", &[("x-request-id", "abc-123")]);
    assert_eq!(
        (echoed.text(), header(&echoed, "x-request-id")),
        ("abc-123", "abc-123")
    );
    let long = "a".repeat(129);
    for bad in ["", "has space", long.as_str(), "tab\there", "é"] {
        let reply = get("/id", &[("x-request-id", bad)]);
        let made = reply.text().to_string();
        assert_ne!(made, bad);
        assert_eq!(made.len(), 16, "{bad:?}");
        assert!(made.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(header(&reply, "x-request-id"), made);
    }
    let a = app.get("/id").text().to_string();
    let b = app.get("/id").text().to_string();
    assert_ne!(a, b);
}

#[test]
fn streams_end_when_their_writer_does() {
    let mut app = client();
    let mut stream = app.get("/stream");
    assert_eq!(header(&stream, "content-type"), "text/plain");
    assert_eq!(app.next_chunk(&mut stream), Some(b"a".to_vec()));
    assert_eq!(app.next_chunk(&mut stream), Some(b"b".to_vec()));
    assert_eq!(app.next_chunk(&mut stream), None);

    let mut events = app.get("/events");
    assert_eq!(header(&events, "content-type"), "text/event-stream");
    assert_eq!(header(&events, "cache-control"), "no-store");
    assert_eq!(header(&events, "x-accel-buffering"), "no");
    assert_eq!(app.next_chunk(&mut events), Some(b"data: one\n\n".to_vec()));
    // Every line is a field of its own, whatever line ending it had.
    assert_eq!(
        app.next_chunk(&mut events),
        Some(b"data: two\ndata: lines\ndata: three\n\n".to_vec())
    );
    assert_eq!(app.next_chunk(&mut events), None);
    // Not a stream: nothing to read.
    let mut page = app.get("/hello");
    assert_eq!(app.next_chunk(&mut page), None);
}

#[test]
fn websockets_need_the_real_server_in_process() {
    // Not an upgrade at all: told what the address takes.
    let plain = get("/ws", &[]);
    assert_eq!(
        (plain.status, header(&plain, "upgrade")),
        (426, "websocket")
    );
    let upgrade = [
        ("upgrade", "websocket"),
        ("connection", "Upgrade"),
        ("sec-websocket-version", "13"),
        ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
    ];
    let reply = get("/ws", &upgrade);
    assert_eq!(reply.status, 501);
    assert_eq!(reply.text(), "WebSockets need Wisp's own server");
}

#[test]
fn handlers_may_wait() {
    assert_eq!(get("/sleep", &[]).text(), "slept");
}

#[test]
fn the_dev_endpoint_swaps_templates_for_loopback_only() {
    let (path, shape) = TEMPLATE;
    let swap = |peer: &str, body: String| {
        let mut req = request("POST", "/_wisp/dev/swap", &[], body.as_bytes());
        req.peer = peer.parse().unwrap();
        let reply = client().send(req);
        (reply.status, reply.text().to_string())
    };
    let good = |shape: u64| format!("{path}\n{shape:x}\n2\n5\nhello3\nabc");
    assert_eq!(wisp::rt::chunk(0, 1, "compiled"), "compiled");

    let strangers = swap("8.8.8.8:1", good(shape));
    assert_eq!(strangers.0, 404, "only this machine may swap");
    assert_eq!(wisp::rt::chunk(0, 1, "compiled"), "compiled");
    assert_eq!(swap("[::1]:1", good(shape)).0, 200);

    assert_eq!(swap("127.0.0.1:1", good(shape)), (200, "swapped".into()));
    assert_eq!(wisp::rt::chunk(0, 0, "compiled"), "hello");
    assert_eq!(wisp::rt::chunk(0, 1, "compiled"), "abc");
    // A chunk it was not sent stays as compiled, and so does another template.
    assert_eq!(wisp::rt::chunk(0, 2, "compiled"), "compiled");
    assert_eq!(wisp::rt::chunk(1, 0, "compiled"), "compiled");
    // Swapping again replaces.
    assert_eq!(
        swap("127.0.0.1:1", format!("{path}\n{shape:x}\n1\n2\nok")).0,
        200
    );
    assert_eq!(wisp::rt::chunk(0, 0, "compiled"), "ok");
    assert_eq!(wisp::rt::chunk(0, 1, "compiled"), "compiled");

    for (body, message) in [
        (good(shape + 1), "template shape changed; rebuild needed"),
        (
            format!("src/other.wisp\n{shape:x}\n0\n"),
            "unknown template",
        ),
        (format!("{path}\n{shape:x}"), "truncated request"),
        (format!("{path}\nxyz\n0\n"), "bad shape"),
        (format!("{path}\n{shape:x}\nmany\n"), "bad count"),
        (format!("{path}\n{shape:x}\n1\nlots\nx"), "bad length"),
        (format!("{path}\n{shape:x}\n1\n9\nshort"), "bad length"),
        (format!("{path}\n{shape:x}\n1\n1\n\u{e9}"), "bad length"),
    ] {
        let got = swap("127.0.0.1:1", body.clone());
        assert_eq!(got, (409, message.into()), "{body:?}");
    }
    let mut not_utf8 = request("POST", "/_wisp/dev/swap", &[], &[0xff, 0xfe]);
    not_utf8.peer = "127.0.0.1:1".parse().unwrap();
    assert_eq!(client().send(not_utf8).text(), "body is not UTF-8");

    let mut app = client();
    assert_eq!(app.get("/_wisp/dev/swap").status, 404);
    assert_eq!(app.get("/_wisp/other").status, 404);
    // No OpenAPI document without endpoints, so the docs are not there either.
    assert_eq!(app.get("/_wisp/openapi.json").status, 404);
    assert_eq!(app.get("/_wisp/docs").status, 404);
}

#[test]
fn a_form_post_must_come_from_this_site() {
    let post =
        |headers: &[(&str, &str)]| client().send(request("POST", "/form", headers, b"")).status;
    let host = ("host", "app.example:3000");
    // Not a browser: it sends no `Origin`.
    assert_eq!(post(&[host]), 200);
    assert_eq!(post(&[host, ("origin", "http://app.example:3000")]), 200);
    assert_eq!(post(&[host, ("origin", "https://APP.example:3000")]), 200);
    for origin in [
        "https://evil.example",
        "null",
        "",
        "http://app.example:30000",
        "http://app.example",
        "http://app.example:3000.evil.example",
        "http://evil.example/app.example:3000",
        "http://evil.example#app.example:3000",
    ] {
        assert_eq!(post(&[host, ("origin", origin)]), 403, "{origin:?}");
    }
    // Behind a proxy that changes `Host`, the first name it forwards is the site's.
    let proxied = |xfh: &str, origin: &str| {
        post(&[
            ("host", "10.0.0.5:3000"),
            ("x-forwarded-host", xfh),
            ("origin", origin),
        ])
    };
    assert_eq!(proxied("app.example", "https://app.example"), 200);
    assert_eq!(proxied("app.example, 10.0.0.9", "https://app.example"), 200);
    assert_eq!(
        proxied("evil.example, app.example", "https://app.example"),
        403
    );
    assert_eq!(proxied("app.example", "https://evil.example"), 403);
    let refused = client().send(request(
        "POST",
        "/form",
        &[host, ("origin", "https://evil.example")],
        b"",
    ));
    assert!(
        refused
            .text()
            .contains("[403:Cross-site form submissions are forbidden]")
    );
}

#[test]
fn settings_and_values_of_the_process() {
    // Cargo sets these for every test.
    assert_eq!(wisp::env("CARGO_PKG_NAME").as_deref(), Some("wisp"));
    assert_eq!(wisp::env("WISP_SURELY_NOT_SET"), None);
    assert_eq!(wisp::env_or("WISP_SURELY_NOT_SET", 7u32), 7);
    assert_eq!(wisp::env_or("CARGO_PKG_NAME", String::new()), "wisp");
    assert_eq!(wisp::env_or("CARGO_PKG_VERSION_MAJOR", 9u32), 0);
    // Set but not a number: a typo is never quietly replaced by the default.
    let typo = std::panic::catch_unwind(|| wisp::env_or("CARGO_PKG_NAME", 1u32));
    assert!(typo.is_err());

    assert!(wisp::secure_eq("token", "token"));
    assert!(wisp::secure_eq(b"", b""));
    assert!(!wisp::secure_eq("token", "Token"));
    assert!(!wisp::secure_eq("token", "token1"));
    assert!(!wisp::secure_eq("", "x"));

    struct Nothing;
    let missing = std::panic::catch_unwind(wisp::state::<Nothing>);
    assert!(
        missing.is_err(),
        "a value nobody provided is named by a panic"
    );
    assert_eq!(wisp::KB, 1024);
    assert_eq!(wisp::MB, 1 << 20);
}

#[test]
fn shared_values_lock_without_unwrap() {
    static COUNT: wisp::Shared<u32> = wisp::Shared::new(0);
    *COUNT.lock() += 2;
    assert_eq!(*COUNT.lock(), 2);
    // A panic while it was held leaves the value as it was, and the lock usable.
    let held = std::thread::spawn(|| {
        let mut guard = COUNT.lock();
        *guard += 1;
        panic!("while held");
    });
    assert!(held.join().is_err());
    assert_eq!(*COUNT.lock(), 3);
}

#[test]
fn a_task_runs_every_period_until_dropped() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let ticks = Arc::new(AtomicU32::new(0));
    runtime.block_on(async {
        let counted = ticks.clone();
        wisp::every(Duration::from_millis(5), move || {
            let counted = counted.clone();
            async move {
                counted.fetch_add(1, Ordering::Relaxed);
            }
        });
        for _ in 0..200 {
            if ticks.load(Ordering::Relaxed) >= 3 {
                return;
            }
            wisp::sleep(Duration::from_millis(5)).await;
        }
        panic!("the task ran {} times", ticks.load(Ordering::Relaxed));
    });
    assert!(std::panic::catch_unwind(|| wisp::every(Duration::ZERO, || async {})).is_err());
}

#[test]
fn errors_are_plain_data() {
    let e = wisp::Error::invalid("title", "is required").and("done", "is not a bool");
    assert_eq!(e.status(), 422);
    assert_eq!(e.message(), "title: is required; done: is not a bool");
    assert_eq!(
        e.fields(),
        [
            ("title".to_string(), "is required".to_string()),
            ("done".to_string(), "is not a bool".to_string())
        ]
    );
    assert_eq!(
        format!("{e:?}"),
        "422 title: is required; done: is not a bool"
    );
    let moved = wisp::Error::redirect(301, "/x");
    assert_eq!(format!("{moved:?}"), "301  (location: /x)");
    let io: wisp::Error = std::io::Error::other("nope").into();
    assert_eq!((io.status(), io.message()), (500, "nope"));
    assert!(format!("{io:?}").starts_with("500 nope ("));
    // Statuses that are not errors, or redirects that are not, are mistakes.
    assert!(std::panic::catch_unwind(|| wisp::Error::new(200, "x")).is_err());
    assert!(std::panic::catch_unwind(|| wisp::Error::new(600, "x")).is_err());
    assert!(std::panic::catch_unwind(|| wisp::Error::redirect(200, "/x")).is_err());
    assert!(std::panic::catch_unwind(|| wisp::Error::redirect(309, "/x")).is_err());
    assert!(std::panic::catch_unwind(|| wisp::Response::empty(99)).is_err());
    assert!(std::panic::catch_unwind(|| wisp::Response::empty(1000)).is_err());

    use wisp::OrStatus;
    assert_eq!(Some(1).or_404().unwrap(), 1);
    assert_eq!(None::<u8>.or_status(410).unwrap_err().status(), 410);
    assert_eq!(None::<u8>.or_status(410).unwrap_err().message(), "Gone");
    assert_eq!("x".parse::<u8>().or_status(400).unwrap_err().status(), 400);
}

#[test]
fn the_test_client_sends_what_a_browser_or_an_api_client_would() {
    let mut app = client();
    // A form as a browser encodes it: spaces as `+`, the rest of what is not plain in `%XX`.
    let form = app.post_form(
        "/echo",
        &[("name", "a b&c=d"), ("\u{e9}", "*-_.~"), ("empty", "")],
    );
    assert_eq!(
        form.text().split_once(':').unwrap().1,
        "name=a+b%26c%3Dd&%C3%A9=*-_.%7E&empty="
    );
    assert_eq!(app.post_form("/echo", &[]).text(), "0:");
    assert_eq!(
        app.post_form("/meta", &[]).text(),
        "POST application/x-www-form-urlencoded - -"
    );

    // JSON, whatever the method.
    assert_eq!(
        app.post_json("/meta", "{}").text(),
        "POST application/json - -"
    );
    assert_eq!(
        app.put_json("/meta", "{}").text(),
        "PUT application/json - -"
    );
    assert_eq!(
        app.patch_json("/meta", "{}").text(),
        "PATCH application/json - -"
    );
    assert_eq!(app.post_json("/echo", r#"{"a":1}"#).text(), r#"7:{"a":1}"#);
    assert_eq!(app.delete("/meta").text(), "DELETE - - -");
    assert_eq!(app.get("/meta").text(), "GET - - -");

    app.bearer("t0k3n");
    assert_eq!(app.get("/meta").text(), "GET - Bearer t0k3n -");

    // Cookies the server set come back on every request, and go when it deletes them.
    app.get("/set-cookie?v=1");
    app.get("/sign?v=ada");
    assert_eq!(app.cookie("a"), Some("1"));
    let user = app.cookie("user").unwrap().to_string();
    let sent = app.get("/meta");
    assert!(
        sent.text().contains(&format!("a=1; user={user}")),
        "{}",
        sent.text()
    );
    app.get("/set-cookie?v=2");
    assert_eq!(
        app.cookie("a"),
        Some("2"),
        "a cookie is replaced, not added"
    );
    app.get("/set-cookie?v=");
    assert_eq!(app.cookie("a"), None);
    assert!(app.get("/meta").text().ends_with(&format!("user={user}")));
    assert_eq!(app.cookie("nope"), None);
}

#[test]
fn messages_are_text_or_bytes() {
    use wisp::Message;
    let text = Message::from("hi");
    assert_eq!((text.text(), text.bytes()), ("hi", &b"hi"[..]));
    let owned = Message::from(String::from("yo"));
    assert_eq!(owned.text(), "yo");
    let binary = Message::from(vec![0xff, 0x00]);
    assert_eq!((binary.text(), binary.bytes()), ("", &[0xff, 0x00][..]));
    let slice = Message::from(&[1u8, 2][..]);
    assert_eq!(slice.bytes(), [1, 2]);
    assert!(matches!(slice, Message::Binary(_)) && matches!(text, Message::Text(_)));
}

#[test]
fn a_header_is_read_as_any_type() {
    // Through the app's `/meta`, which reads none: `header_or` is on `Cx`, so check it on a built one.
    let cx = wisp::Cx::from_request::<Lab>(
        "GET",
        "/",
        [("x-n", &b" 42 "[..]), ("x-bad", &b"forty"[..])],
        b"",
        "127.0.0.1:1".parse().unwrap(),
    )
    .unwrap();
    assert_eq!(
        cx.header_or("x-n", 0u32),
        42,
        "surrounding space is not part of a number"
    );
    assert_eq!(cx.header_or("x-bad", 7u32), 7);
    assert_eq!(cx.header_or("x-missing", 8u32), 8);
    assert_eq!(
        cx.header_or("X-N", 0i64),
        42,
        "names are not case sensitive"
    );
    assert_eq!(cx.query_or("n", 3u8), 3);
    assert_eq!(cx.request_id().len(), 16);
    // A parameter the route does not have is a mistake in the code, named in the panic.
    let missing = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cx.param("nope").len()));
    assert!(missing.is_err());
}

/// A dev build keeps its secret in a file, and only its owner may read it:
/// with the secret, anyone can sign in as anyone. (A file left by an older
/// version is closed to others the next time it is read.)
#[cfg(unix)]
#[test]
fn the_dev_secret_is_readable_by_its_owner_alone() {
    use std::os::unix::fs::PermissionsExt;
    if !cfg!(debug_assertions) || wisp::env("WISP_SECRET").is_some() {
        return; // release builds and operators bring their own
    }
    let mut app = client();
    assert_eq!(app.get("/sign?v=ada").status, 200);
    let file = Path::new(ROOT).join(".wisp/secret");
    let mode = std::fs::metadata(&file).unwrap().permissions().mode();
    assert_eq!(mode & 0o077, 0, "{file:?} is {mode:o}");
    assert!(std::fs::read_to_string(&file).unwrap().trim().len() >= 32);
}
