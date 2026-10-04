//! Answers sent as bytes made before, on the wire: a page that reads
//! nothing of the request, baked at build time, and responses `CACHE`
//! keeps.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use common::{Server, body, header, start, status};

/// Out of dev mode (which bakes and keeps nothing, its templates changing
/// under it), with one worker, so every connection finds what it kept.
fn server() -> Server {
    start(&[("WISP_DEV", "off"), ("WISP_THREADS", "1")])
}

fn get(s: &Server, target: &str, headers: &str) -> String {
    s.request("GET", target, headers, b"")
}

#[test]
fn a_page_that_reads_nothing_is_baked_whole() {
    let s = server();
    let page = get(&s, "/baked", "");
    assert_eq!(status(&page), 200, "{page}");
    assert_eq!(
        header(&page, "content-type"),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(header(&page, "x-app"), Some("test"), "`before` still runs");
    let len: usize = header(&page, "content-length").unwrap().parse().unwrap();
    assert_eq!(body(&page).len(), len);
    for want in [
        "<title>Baked</title>",
        "<nav class=\"baked\">Tom &amp; Jerry</nav>",
        "<h1>&lt;made&gt; at build time, 42 of them</h1>",
        "<h2>Plain ★</h2>",
        "<p>3 items</p>",
        "<span class=\"badge\">new</span>",
    ] {
        assert!(page.contains(want), "{want}\n{page}");
    }

    // The bytes a render makes (dev mode renders), for each page the build
    // baked: this one, a form, a route whose parameter goes unread.
    let dev = start(&[("WISP_DEV", "on")]);
    for path in ["/baked", "/login", "/match/opt/7"] {
        let (baked, rendered) = (get(&s, path, ""), get(&dev, path, ""));
        assert!(header(&baked, "etag").is_some(), "{path} is baked");
        assert_eq!(
            (status(&baked), body(&baked)),
            (200, body(&rendered)),
            "{path}"
        );
    }

    // Its ETag was made at build time: a client that has it gets a 304.
    let etag = header(&page, "etag").expect("an etag");
    let again = get(&s, "/baked", &format!("if-none-match: W/\"x\", {etag}\r\n"));
    assert_eq!(
        (status(&again), header(&again, "etag"), body(&again)),
        (304, Some(etag), "")
    );
    assert_eq!(header(&again, "x-app"), Some("test"));
    assert_eq!(status(&get(&s, "/baked", "if-none-match: \"x\"\r\n")), 200);

    // HEAD: the head alone, with the length of the page.
    let head = s.request("HEAD", "/baked", "", b"");
    assert_eq!(
        (status(&head), header(&head, "content-length"), body(&head)),
        (200, Some(&*len.to_string()), "")
    );
}

#[test]
fn cache_keeps_a_render_for_requests_without_cookies() {
    let s = server();
    let first = get(&s, "/cached", "");
    assert!(first.contains("<p>render 0</p>"), "{first}");
    assert_eq!(header(&first, "x-app"), Some("test"));
    assert_eq!(body(&get(&s, "/cached", "")), body(&first), "kept");
    let etag = header(&first, "etag").expect("an etag");
    let again = get(&s, "/cached", &format!("if-none-match: {etag}\r\n"));
    assert_eq!((status(&again), body(&again)), (304, ""));

    // A cookie or credentials could make the page theirs: it renders for
    // them, and what is kept stays as it was.
    let signed = get(&s, "/cached", "cookie: a=1\r\n");
    assert!(signed.contains("render 1"), "{signed}");
    assert_eq!(header(&signed, "etag"), None);
    let bearer = get(&s, "/cached", "authorization: Bearer x\r\n");
    assert!(bearer.contains("render 2"), "{bearer}");
    assert_eq!(body(&get(&s, "/cached", "")), body(&first));

    // Each query is its own, and HEAD is answered from what is kept.
    assert!(get(&s, "/cached?page=2", "").contains("render 3"));
    assert!(get(&s, "/cached?page=2", "").contains("render 3"));
    let head = s.request("HEAD", "/cached?page=2", "", b"");
    assert_eq!((status(&head), body(&head)), (200, ""));

    // A response that sets a cookie is its visitor's: never kept.
    let seen = get(&s, "/cached?cookie=1", "");
    assert!(seen.contains("render 4"), "{seen}");
    assert!(header(&seen, "set-cookie").is_some());
    assert!(get(&s, "/cached?cookie=1", "").contains("render 5"));
}

#[test]
fn cache_public_keeps_for_every_request() {
    let s = server();
    let first = get(&s, "/kept", "");
    assert_eq!(body(&first), "\"call 0\"");
    assert_eq!(header(&first, "content-type"), Some("application/json"));
    for headers in ["", "cookie: a=1\r\n", "authorization: Bearer x\r\n"] {
        assert_eq!(body(&get(&s, "/kept", headers)), "\"call 0\"", "{headers}");
    }
}

/// `const PRERENDER: bool = true;`, before `wisp build` renders it into
/// the binary: the first render is kept for good, for every request.
#[test]
fn a_prerendered_page_renders_once() {
    let s = server();
    let first = get(&s, "/prerendered", "");
    assert!(first.contains("<p>render 0</p>"), "{first}");
    for headers in ["", "cookie: a=1\r\n", "authorization: Bearer x\r\n"] {
        assert_eq!(
            body(&get(&s, "/prerendered", headers)),
            body(&first),
            "{headers}"
        );
    }
    let etag = header(&first, "etag").expect("an etag");
    let again = get(&s, "/prerendered", &format!("if-none-match: {etag}\r\n"));
    assert_eq!((status(&again), body(&again)), (304, ""));
}

#[test]
fn dev_mode_keeps_nothing() {
    let s = start(&[("WISP_DEV", "on"), ("WISP_THREADS", "1")]);
    assert!(get(&s, "/cached", "").contains("render 0"));
    assert!(get(&s, "/cached", "").contains("render 1"));
    assert_eq!(header(&get(&s, "/baked", ""), "etag"), None, "rendered");
}

/// Pages carry the policy: rendered, baked and kept alike, with the hash
/// of each inline script the app writes and what `init` changed. Dev mode
/// adds esm.sh, for npm modules; endpoints get none.
#[test]
fn pages_carry_the_content_security_policy() {
    let s = server();
    let page = get(&s, "/csp", "");
    let policy = header(&page, "content-security-policy").expect("a policy");
    assert!(
        policy.starts_with("default-src 'self'; script-src 'self' 'sha256-"),
        "{policy}"
    );
    // `printf 'document.title = "ran"' | openssl dgst -sha256 -binary | base64`
    let hash = " 'sha256-i8NQMUzpim5Tk3+GzgA/+tCZSay6b48zbjIG0IpKqww='";
    assert!(policy.contains(hash), "{policy}");
    assert!(
        policy.contains("; img-src 'self' https://img.example; ")
            && policy.ends_with("; base-uri 'self'; form-action 'self'; frame-ancestors 'self'"),
        "{policy}"
    );
    assert!(!policy.contains("esm.sh"), "{policy}");
    assert!(header(&page, "etag").is_some(), "baked");
    for path in ["/cached", "/cached", "/login", "/nope"] {
        let other = get(&s, path, "accept: text/html\r\n");
        assert_eq!(
            header(&other, "content-security-policy"),
            Some(policy),
            "{path}"
        );
    }
    assert_eq!(
        header(&get(&s, "/kept", ""), "content-security-policy"),
        None
    );
    let dev = start(&[("WISP_DEV", "on")]);
    let page = get(&dev, "/csp", "");
    let policy = header(&page, "content-security-policy").unwrap();
    assert!(policy.contains("https://esm.sh"), "{policy}");
}
