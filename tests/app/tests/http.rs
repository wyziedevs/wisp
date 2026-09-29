//! The test app's binary, run as a server: hooks, state, signed cookies,
//! uploads, actions that answer with a file, chunked request bodies, body
//! limits, streamed responses and browser code (client scripts and
//! directives), each checked on the wire.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// The server, stopped when dropped.
struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start() -> Server {
    let mut child = Command::new(env!("CARGO_BIN_EXE_wisp-test-app"))
        .env("PORT", "0")
        .env("HOST", "127.0.0.1")
        .env("WISP_THREADS", "2")
        .env("WISP_SECRET", "0123456789abcdef0123456789abcdef")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start the test app");
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let port = line
        .trim()
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or_else(|| panic!("no port in {line:?}"));
    Server { child, port }
}

impl Server {
    /// Sends `raw` as is and returns everything that comes back until the
    /// server closes the connection.
    fn send(&self, raw: &[u8]) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(raw).unwrap();
        let mut got = Vec::new();
        let _ = s.read_to_end(&mut got);
        String::from_utf8_lossy(&got).into_owned()
    }

    /// One request that closes the connection when answered.
    fn request(&self, method: &str, target: &str, headers: &str, body: &[u8]) -> String {
        let mut raw = format!(
            "{method} {target} HTTP/1.1\r\nhost: 127.0.0.1:{}\r\nconnection: close\r\ncontent-length: {}\r\n{headers}\r\n",
            self.port,
            body.len()
        )
        .into_bytes();
        raw.extend_from_slice(body);
        self.send(&raw)
    }
}

fn status(response: &str) -> u16 {
    response
        .get(9..12)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

fn header<'a>(response: &'a str, name: &str) -> Option<&'a str> {
    let head = response.split("\r\n\r\n").next()?;
    head.lines().find_map(|l| {
        l.split_once(": ")
            .filter(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    })
}

fn body(response: &str) -> &str {
    response.split_once("\r\n\r\n").map_or("", |(_, b)| b)
}

const FORM: &str = "content-type: application/x-www-form-urlencoded\r\n";

#[test]
fn hooks_and_state() {
    let s = start();
    let home = s.request("GET", "/", "", b"");
    assert_eq!(status(&home), 200);
    assert!(home.contains("<h1>hello from init</h1>"), "{home}");
    assert_eq!(header(&home, "x-app"), Some("test"));

    // What `before` set stays on an error page.
    let missing = s.request("GET", "/nope", "", b"");
    assert_eq!(
        (status(&missing), header(&missing, "x-app")),
        (404, Some("test"))
    );

    // `before` can answer by itself: a CORS preflight, for any path.
    let preflight = s.request("OPTIONS", "/echo", "", b"");
    assert_eq!(
        (
            status(&preflight),
            header(&preflight, "access-control-allow-origin")
        ),
        (204, Some("*"))
    );
}

#[test]
fn components() {
    let s = start();
    let home = s.request("GET", "/", "", b"");
    let cards: Vec<&str> = home
        .split("<section class=\"card\">")
        .skip(1)
        .map(|c| c.split("</section>").next().unwrap())
        .collect();
    assert_eq!(cards.len(), 2, "{home}");
    // Props given, a flag, children with components of their own.
    assert!(
        cards[0].contains("<h2>hello from init ★</h2>") && cards[0].contains("<p>3 items</p>"),
        "{}",
        cards[0]
    );
    assert!(
        cards[0].contains("<span class=\"badge\">new</span>")
            && cards[0].contains("<span class=\"badge\">15</span>"),
        "{}",
        cards[0]
    );
    // Defaults, and no children.
    assert!(
        cards[1].contains("<h2>Plain</h2>")
            && cards[1].contains("<p>0 items</p>")
            && !cards[1].contains("badge"),
        "{}",
        cards[1]
    );
}

#[test]
fn signed_cookies_sign_in() {
    let s = start();
    let away = s.request("GET", "/admin", "", b"");
    assert_eq!(
        (status(&away), header(&away, "location")),
        (303, Some("/login"))
    );

    let login = s.request("POST", "/login", FORM, b"name=ada");
    assert_eq!(
        (status(&login), header(&login, "location")),
        (303, Some("/admin"))
    );
    let cookie = header(&login, "set-cookie").unwrap();
    let value = cookie.split(';').next().unwrap();
    assert!(value.starts_with("user=ada."), "{cookie}");

    let admin = s.request("GET", "/admin", &format!("cookie: {value}\r\n"), b"");
    assert_eq!(status(&admin), 200);
    assert!(admin.contains("Welcome, ada"));
    assert!(
        s.request("GET", "/", &format!("cookie: {value}\r\n"), b"")
            .contains("Signed in as ada")
    );

    // Another name under ada's signature, or no signature, is nobody.
    let mac = value.rsplit('.').next().unwrap();
    for forged in [
        format!("user=bob.{mac}"),
        "user=ada".into(),
        format!("user=ada.{mac}x"),
    ] {
        let r = s.request("GET", "/admin", &format!("cookie: {forged}\r\n"), b"");
        assert_eq!(status(&r), 303, "{forged}");
    }
}

/// A multipart body with a title and, if given, a file.
fn multipart(file: Option<(&str, &[u8])>) -> (String, Vec<u8>) {
    let mut b =
        b"--XX\r\ncontent-disposition: form-data; name=\"title\"\r\n\r\nMy cat\r\n".to_vec();
    if let Some((name, bytes)) = file {
        b.extend_from_slice(format!("--XX\r\ncontent-disposition: form-data; name=\"photo\"; filename=\"{name}\"\r\ncontent-type: image/png\r\n\r\n").as_bytes());
        b.extend_from_slice(bytes);
        b.extend_from_slice(b"\r\n");
    }
    b.extend_from_slice(b"--XX--\r\n");
    (
        "content-type: multipart/form-data; boundary=XX\r\n".into(),
        b,
    )
}

#[test]
fn uploads() {
    let s = start();
    let photo = vec![7u8; 2 * 1024 * 1024]; // over the default 1 MB, under the route's 4 MB
    let (ct, b) = multipart(Some(("cat.png", &photo)));
    let r = s.request("POST", "/upload", &ct, &b);
    assert_eq!(status(&r), 200, "{}", &r[..r.len().min(300)]);
    assert!(r.contains("cat.png: 2097152 bytes of image/png"), "{r}");

    // No file: the action says so with a 422, and the page keeps the title.
    let (ct, b) = multipart(None);
    let r = s.request("POST", "/upload", &ct, &b);
    assert_eq!(status(&r), 422);
    assert!(
        r.contains("Choose a photo") && r.contains("value=\"My cat\""),
        "{r}"
    );

    // An action can answer with a file instead of the page.
    let r = s.request("POST", "/upload?/export", &ct, &b);
    assert_eq!(
        (status(&r), header(&r, "content-type")),
        (200, Some("text/csv"))
    );
    assert_eq!(body(&r), "title\nMy cat\n");

    // Over the route's limit, refused before the body is read.
    let r = s.send(
        format!(
            "POST /upload HTTP/1.1\r\nhost: x\r\n{ct}content-length: {}\r\n\r\n",
            5 * 1024 * 1024
        )
        .as_bytes(),
    );
    assert_eq!(status(&r), 413);
}

#[test]
fn body_limits_are_per_route() {
    let s = start();
    let big = vec![b'a'; 100 * 1024];
    assert_eq!(
        status(&s.request("POST", "/echo", "", &big)),
        413,
        "over echo's 64 KB"
    );
    assert_eq!(body(&s.request("POST", "/echo", "", b"hi")), "2:hi");
    // Another route keeps the default 1 MB: this one is refused for its method, not its size.
    assert_eq!(status(&s.request("POST", "/", "", &big)), 405);
}

#[test]
fn chunked_request_bodies() {
    let s = start();
    let r = s.send(b"POST /echo HTTP/1.1\r\nhost: x\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n5\r\nhello\r\n6;ext=1\r\n world\r\n0\r\ntrailer: x\r\n\r\n");
    assert_eq!((status(&r), body(&r)), (200, "11:hello world"));

    // Pipelined after a chunked request, the next is read from where it ended.
    let two = s.send(b"POST /echo HTTP/1.1\r\nhost: x\r\ntransfer-encoding: chunked\r\n\r\n1\r\na\r\n0\r\n\r\nPOST /echo HTTP/1.1\r\nhost: x\r\ncontent-length: 1\r\nconnection: close\r\n\r\nb");
    assert!(two.contains("1:a") && two.contains("1:b"), "{two}");

    let bad: [(&[u8], u16); 5] = [
        (b"5\r\nhel\r\n0\r\n\r\n", 400),    // shorter than it says
        (b"zz\r\nhello\r\n0\r\n\r\n", 400), // not hex
        (b"5\nhello\r\n0\r\n\r\n", 400),    // bare LF
        (b"10001\r\n", 413),                // over echo's 64 KB
        (b"fffffffffffffffffff\r\n", 400),  // more digits than any size has
    ];
    for (chunks, want) in bad {
        let mut raw =
            b"POST /echo HTTP/1.1\r\nhost: x\r\ntransfer-encoding: chunked\r\n\r\n".to_vec();
        raw.extend_from_slice(chunks);
        assert_eq!(
            status(&s.send(&raw)),
            want,
            "{:?}",
            String::from_utf8_lossy(chunks)
        );
    }
    let gzip = s.send(
        b"POST /echo HTTP/1.1\r\nhost: x\r\ntransfer-encoding: gzip, chunked\r\n\r\n0\r\n\r\n",
    );
    assert_eq!(status(&gzip), 501);
    let old = s.send(b"POST /echo HTTP/1.0\r\ntransfer-encoding: chunked\r\n\r\n0\r\n\r\n");
    assert_eq!(status(&old), 400);
}

#[test]
fn server_sent_events() {
    let s = start();
    let r = s.send(b"GET /events HTTP/1.1\r\nhost: x\r\n\r\nGET / HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n");
    assert_eq!(status(&r), 200);
    assert_eq!(header(&r, "content-type"), Some("text/event-stream"));
    assert_eq!(header(&r, "transfer-encoding"), Some("chunked"));
    assert_eq!(header(&r, "cache-control"), Some("no-store"));
    for i in 0..3 {
        assert!(
            r.contains(&format!("data: tick {i}\ndata: line two\n\n")),
            "{r}"
        );
    }
    // The stream ended properly, and the connection went on to the next request.
    let (events, next) = r.split_once("0\r\n\r\n").unwrap();
    assert!(!events.is_empty());
    assert!(
        next.starts_with("HTTP/1.1 200 OK") && next.contains("hello from init"),
        "{next}"
    );

    // HTTP/1.0 has no chunks: the body runs to the end of the connection.
    let old = s.send(b"GET /events HTTP/1.0\r\n\r\n");
    assert_eq!(
        (
            header(&old, "transfer-encoding"),
            header(&old, "connection")
        ),
        (None, Some("close"))
    );
    assert!(old.ends_with("data: tick 2\ndata: line two\n\n"), "{old}");
}

#[test]
fn files_from_a_directory() {
    let s = start();
    let card = s.request("GET", "/files/Card.wisp", "", b"");
    assert_eq!(status(&card), 200);
    assert!(body(&card).starts_with("{@props title: &str"), "{card}");
    for outside in [
        "/files/..%2FCargo.toml",
        "/files/%2E%2E/Cargo.toml",
        "/files/C:%5Cwindows",
        "/files/nope.wisp",
        "/files/",
    ] {
        let r = s.request("GET", outside, "", b"");
        assert!(!r.contains("[package]"), "{outside}");
        assert!(matches!(status(&r), 404 | 308), "{outside}: {r}");
    }
}

#[test]
fn browser_code() {
    let s = start();
    let page = s.request("GET", "/live", "", b"");
    assert_eq!(status(&page), 200, "{page}");

    // Directives leave the HTML; each element is marked with its instance
    // (the page is 0, the component in it 1) and its group.
    assert!(
        !page.contains("on:click") && !page.contains(":hidden") && !page.contains("bind:"),
        "{page}"
    );
    assert!(
        page.contains("<h1 data-w=\"0.0\">Shop &amp; &lt;save&gt;</h1>"),
        "{page}"
    );
    assert!(
        page.contains("<button data-w=\"0.1\">Menu</button>"),
        "{page}"
    );
    assert!(page.contains("<ul hidden data-w=\"0.2\">"), "{page}");
    assert!(page.contains("<input data-w=\"0.4\">"), "{page}");
    // A Rust loop's values that a directive reads, as HTML-escaped JSON.
    assert!(page.contains("<li data-w=\"0.3\" data-wl=\"{&quot;item&quot;:{&quot;name&quot;:&quot;tea&quot;}}\">0: tea</li>"), "{page}");
    assert!(page.contains("data-wl=\"{&quot;item&quot;:{&quot;name&quot;:&quot;cake \\&quot;big\\&quot;&quot;}}\">1: cake &quot;big&quot;</li>"), "{page}");
    // A client `<template each>`: the elements inside have no instance.
    assert!(
        page.contains("<template data-w=\"0.6\"><p data-w=\"7\"></p></template>"),
        "{page}"
    );
    assert!(
        page.contains("<button data-w=\"1.0\">More</button>")
            && page.contains("<div hidden data-w=\"1.1\">Menu</div>"),
        "{page}"
    );
    // The client script is not in the page.
    assert!(!page.contains("let open"), "{page}");

    // The instances and their modules end the body.
    let json_at = page
        .find("<script type=\"application/json\" id=\"wisp-live\">")
        .expect("the instances");
    assert!(
        json_at < page.find("</body>").unwrap() && json_at > page.find("More</button>").unwrap()
    );
    let json = &page[json_at..];
    let json = &json[json.find('>').unwrap() + 1..json.find("</script>").unwrap()];
    let modules: Vec<(&str, &str)> = json["{\"m\":{".len()..json.find('}').unwrap()]
        .split(',')
        .map(|kv| {
            kv.split_once(':')
                .map(|(k, v)| (k.trim_matches('"'), v.trim_matches('"')))
                .unwrap()
        })
        .collect();
    assert_eq!(modules.len(), 2, "{json}");
    let (page_id, page_url) = modules[0];
    let (comp_id, comp_url) = modules[1];
    assert!(
        page_url.starts_with(&format!("/_app/c/{page_id}.js?v=")),
        "{json}"
    );
    assert!(
        json.ends_with(&format!(
            "\"i\":[[0,\"{page_id}\",-1,{{\"data\":{{\"title\":\"Shop \\u0026 \\u003csave\\u003e\"}}}}],[1,\"{comp_id}\",0,{{\"label\":\"More\"}}]]}}"
        )),
        "{json}"
    );
    for url in [page_url, comp_url] {
        assert!(
            page.contains(&format!("<link rel=\"modulepreload\" href=\"{url}\">")),
            "{page}"
        );
    }
    let runtime = format!(
        "<script type=\"module\" src=\"/_app/live.js?v={}\"></script>",
        env!("CARGO_PKG_VERSION")
    );
    assert!(page.contains(&format!("{runtime}\n</body>")), "{page}");

    // The modules, and the runtime they import.
    let module = s.request("GET", page_url, "", b"");
    assert_eq!(
        (status(&module), header(&module, "content-type")),
        (200, Some("text/javascript; charset=utf-8"))
    );
    assert_eq!(
        header(&module, "cache-control"),
        Some("public, max-age=31536000, immutable")
    );
    let js = body(&module);
    assert!(js.starts_with(&format!("import {{ define }} from \"/_app/live.js?v={}\";\ndefine(\"{page_id}\", function (__wisp_p, __wisp_h) {{ const {{", env!("CARGO_PKG_VERSION"))), "{js}");
    assert!(js.contains("let { data } = __wisp_p;"), "{js}");
    assert!(
        js.contains("[\"on\", \"click\", [\"prevent\"], ({ item }, event) => (pick(item.name))]"),
        "{js}"
    );
    assert!(
        js.contains("[\"each\", () => (notes), [\"note\", \"n\"], null]"),
        "{js}"
    );
    assert!(
        js.ends_with("//# sourceURL=wisp:///src/routes/live/+page.wisp\n"),
        "{js}"
    );
    // The script keeps its line numbers: `let open` is on line 14 of the file.
    assert_eq!(
        js.lines().position(|l| l.trim() == "let open = false"),
        Some(13),
        "{js}"
    );
    let etag = header(&module, "etag").unwrap().to_string();
    assert_eq!(
        status(&s.request("GET", page_url, &format!("if-none-match: {etag}\r\n"), b"")),
        304
    );
    assert!(body(&s.request("GET", comp_url, "", b"")).contains("define(\""));
    let live = s.request(
        "GET",
        &format!("/_app/live.js?v={}", env!("CARGO_PKG_VERSION")),
        "",
        b"",
    );
    assert_eq!(
        (status(&live), header(&live, "content-type")),
        (200, Some("text/javascript; charset=utf-8"))
    );
    assert!(body(&live).contains("export function define"));
    assert_eq!(status(&s.request("GET", "/_app/c/t999.js", "", b"")), 404);

    // A page without browser code has none of this.
    assert!(!s.request("GET", "/", "", b"").contains("wisp-live"));
}

#[test]
fn form_origin_behind_a_proxy() {
    let s = start();
    // A proxy that sends its own Host and the visitor's in X-Forwarded-Host.
    let proxied = format!("{FORM}origin: https://example.com\r\nx-forwarded-host: example.com\r\n");
    let r = s.request("POST", "/login", &proxied, b"name=ada");
    assert_eq!(status(&r), 303);
    let foreign =
        format!("{FORM}origin: https://evil.example\r\nx-forwarded-host: example.com\r\n");
    assert_eq!(
        status(&s.request("POST", "/login", &foreign, b"name=ada")),
        403
    );
}

/// The `n`th module URL in a page's `#wisp-live` JSON.
fn module_url(page: &str, n: usize) -> String {
    let json = &page[page.find("id=\"wisp-live\">").expect("instances")..];
    let url = json.split("\"/_app/c/").nth(n + 1).expect("a module");
    format!("/_app/c/{}", &url[..url.find('"').unwrap()])
}

#[test]
fn client_parity() {
    let s = start();
    let v = env!("CARGO_PKG_VERSION");

    // `{:expr}`: an anchor, what the server knows of it (a server value's
    // path), and an end. A live attribute keeps its static text. Client
    // blocks are templates whose elements bind per copy.
    let holes = s.request("GET", "/a2/holes", "", b"");
    assert!(
        holes.contains("<h1 id=\"title\"><template data-w=\"0.0\"></template><!----></h1>"),
        "{holes}"
    );
    assert!(
        holes.contains("<template data-w=\"0.17\"></template>Hello &lt;server&gt;<!----></p>"),
        "{holes}"
    );
    assert!(
        holes.contains("<p id=\"greet\" class=\"card \" data-w=\"0.1\">"),
        "{holes}"
    );
    assert!(
        holes.contains(
            "<template data-w=\"0.8\"><li data-w=\"9\"><template data-w=\"10\"></template><!---->: "
        ),
        "{holes}"
    );
    let js = body(&s.request("GET", &module_url(&holes, 0), "", b"")).to_string();
    for want in [
        "[\"hole\", () => (title)]",
        "[\"attr\", \"class\", () => (`card ${(mood) ?? ''}`)]",
        "[\"attr\", \"aria-expanded\", () => (open)]",
        "[\"each\", () => (items), [\"item\", \"i\"], ({ item, i }) => (item.id)]",
        "[\"animate\", \"flip\", null]",
        "[\"if\", () => (![...(items ?? [])].length)]",
        "[\"if\", () => (!(open) && (name))]",
        "[\"if\", () => (!(open) && !(name))]",
    ] {
        assert!(js.contains(want), "{want} in {js}");
    }

    // Instances record the one they render inside, for context.
    let state = s.request("GET", "/a2/state", "", b"");
    assert!(
        state.contains(",0,{}],[2,\"") && state.contains("<div id=\"fresh\" data-wisp-reset>"),
        "{state}"
    );

    // A component the browser renders: the page's module imports its
    // module, which carries its markup, slot and all.
    let comps = s.request("GET", "/a2/comps", "", b"");
    let js = body(&s.request("GET", &module_url(&comps, 0), "", b"")).to_string();
    assert!(js.contains("[[\"count\", ({ name }) => (counts[name]), ({ name }, __wisp_v) => { (counts[name]) = __wisp_v }]], [[\"bump\", (_, event) => (bumped = event)]]]"), "{js}");
    let item = js
        .lines()
        .find_map(|l| l.strip_prefix("import \""))
        .map(|l| l.trim_end_matches("\";"))
        .expect("the component's module");
    let item = body(&s.request("GET", item, "", b"")).to_string();
    assert!(
        item.contains("let { label, count } = __wisp_p;")
            && item.contains("<template data-wslot></template>")
            && item.contains("{ html: \""),
        "{item}"
    );

    // `$lib` files: one hash for all, imports rewritten to the same URLs.
    let stores = s.request("GET", "/a2/stores", "", b"");
    let js = body(&s.request("GET", &module_url(&stores, 0), "", b"")).to_string();
    let lib = js
        .split("from \"")
        .nth(2)
        .and_then(|r| r.split('"').next())
        .expect("the lib import")
        .to_string();
    assert!(lib.starts_with("/_app/c/lib/cart.js?v="), "{js}");
    let cart = s.request("GET", &lib, "", b"");
    assert_eq!(
        header(&cart, "cache-control"),
        Some("public, max-age=31536000, immutable")
    );
    let hash = &lib[lib.find("?v=").unwrap()..];
    assert!(
        body(&cart).contains(&format!("from \"/_app/live.js?v={v}\""))
            && body(&cart).contains(&format!("from \"/_app/c/lib/names.js{hash}\"")),
        "{cart}"
    );

    // `+page.js`: the page gets all of `data` for its `load`.
    let load = s.request("GET", "/a2/load", "", b"");
    assert!(
        load.contains("{\"data\":{\"server\":\"server\"}}"),
        "{load}"
    );
    let js = body(&s.request("GET", &module_url(&load, 0), "", b"")).to_string();
    let url = js
        .split("import * as __wisp_u from \"")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .expect("the load module")
        .to_string();
    assert!(js.contains("{ load: __wisp_u.load }"), "{js}");
    assert!(body(&s.request("GET", &url, "", b"")).contains("export async function load"));

    // Browser code that fails to start asks for the route's error page.
    let err = s.request("GET", "/a2/holes", "x-wisp-error: 1\r\n", b"");
    assert_eq!(status(&err), 500);
    assert!(err.contains("<h1 id=\"err\">Error 500</h1>"), "{err}");
}

#[test]
fn template_shorthands() {
    let s = start();
    let page = s.request("GET", "/sugar", "", b"");
    let html = body(&page);
    // `class:` joins the static classes, the shorthand is `href={href}`, a
    // `None` leaves its attribute out, a URL is still guarded.
    for want in [
        "<h1 class=\"big on\">a&lt;b</h1>",
        "<p class=\"on\">7</p>",
        "<a href=\"/x?a=1&amp;b=2\" title=\"hi &quot;you&quot;\" class=\"link a&lt;b\" hidden>link</a>",
        "<a href=\"about:invalid#blocked\" rel=\"x\">bad</a>",
    ] {
        assert!(html.contains(want), "{want}\n{html}");
    }
    let api = s.request("GET", "/sugar/api", "", b"");
    assert_eq!(header(&api, "content-type"), Some("application/json"));
    assert_eq!(body(&api), "[[\"a\\u003cb\",1],[\"c\",null]]");
}
