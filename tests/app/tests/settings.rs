//! The settings an operator gives through the environment (`PORT`, `HOST`,
//! `WISP_*`, `ORIGIN`), on the test app's binary: each one that is not valid
//! stops the server with a message, and each one that is takes effect.

mod common;

use common::{Server, Temp, command, start};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

/// Waits for `child` to exit, or fails after a few seconds.
fn exits(child: &mut Child) -> std::process::ExitStatus {
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    panic!("the process did not exit");
}

/// What a process that is not meant to start says as it stops, and its exit
/// code. Its output goes to files, so a full pipe cannot hold it up.
fn stops(env: &[(&str, &str)]) -> (Option<i32>, String) {
    static RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let file = |what: &str| Temp::new(&format!("{what}-{n}"));
    let (out, err) = (file("out"), file("err"));
    let mut child = command(env)
        .stdout(std::fs::File::create(&*out).unwrap())
        .stderr(std::fs::File::create(&*err).unwrap())
        .spawn()
        .unwrap();
    let status = exits(&mut child);
    let mut said = std::fs::read_to_string(&*err).unwrap();
    said.push_str(&std::fs::read_to_string(&*out).unwrap());
    (status.code(), said)
}

#[test]
fn a_setting_that_is_not_valid_stops_the_server_and_says_why() {
    for (env, said) in [
        (
            ("PORT", "abc"),
            "PORT is \"abc\", which is not a port number from 0 to 65535",
        ),
        (("PORT", "70000"), "PORT is \"70000\""),
        (("PORT", "-1"), "PORT is \"-1\""),
        (
            ("HOST", "not_a_host_at_all"),
            "HOST is \"not_a_host_at_all\", which is not an IP address or a name",
        ),
        (
            ("WISP_THREADS", "0"),
            "WISP_THREADS is 0, which is not a number of threads above 0",
        ),
        (("WISP_THREADS", "x"), "WISP_THREADS is \"x\""),
        (
            ("WISP_SECRET", "short"),
            "WISP_SECRET is 5 characters, too short",
        ),
        (
            ("ORIGIN", "nope"),
            "ORIGIN is \"nope\", which is not a site's address",
        ),
        (
            ("ORIGIN", "https://example.com/path"),
            "ORIGIN is \"https://example.com/path\"",
        ),
        (("ORIGIN", "https://"), "ORIGIN is \"https:\""),
        (("ORIGIN", "https://user@example.com"), "ORIGIN is"),
        (
            ("WISP_BODY_LIMIT", "x"),
            "WISP_BODY_LIMIT is \"x\", which is not a size",
        ),
        (("WISP_BODY_LIMIT", "-5"), "WISP_BODY_LIMIT is \"-5\""),
        (("WISP_BODY_LIMIT", "1TB"), "WISP_BODY_LIMIT is \"1TB\""),
        (
            ("WISP_LOG", "yaml"),
            "WISP_LOG is \"yaml\", which is not json or off",
        ),
        (
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "https://otel.example"),
            "Wisp sends OTLP over plain http://",
        ),
        (
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://"),
            "which is not an http:// address",
        ),
        (("WISP_MAX_CONNS", "x"), "WISP_MAX_CONNS is \"x\""),
        (("WISP_WS_IDLE", "x"), "WISP_WS_IDLE is \"x\""),
        (
            ("WISP_API_DOCS", "maybe"),
            "WISP_API_DOCS is \"maybe\", which is not on or off",
        ),
        (
            ("WISP_REQUEST_ID", "maybe"),
            "WISP_REQUEST_ID is \"maybe\", which is not on or off",
        ),
    ] {
        let (code, out) = stops(&[env]);
        assert_eq!(code, Some(1), "{env:?}: {out}");
        assert!(out.contains(said), "{env:?}: {out}");
        assert!(!out.contains("listening"), "{env:?}");
    }
}

#[test]
fn a_port_in_use_says_what_to_do() {
    let first = start(&[]);
    let (code, out) = stops(&[("PORT", &first.port.to_string())]);
    assert_eq!(code, Some(1), "{out}");
    assert!(
        out.contains(&format!("cannot listen on 127.0.0.1:{}", first.port)),
        "{out}"
    );
    assert!(
        out.contains("Another program, maybe another copy of this one, is using the port"),
        "{out}"
    );
    assert!(out.contains("set PORT to use another"), "{out}");
}

/// One request's answer: status, headers and body.
struct Answer {
    status: u16,
    head: String,
    body: String,
}

impl Answer {
    fn header(&self, name: &str) -> Option<&str> {
        self.head.lines().skip(1).find_map(|l| {
            l.split_once(": ")
                .filter(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| v)
        })
    }
}

impl Answer {
    fn of(got: &str) -> Answer {
        let (head, body) = got.split_once("\r\n\r\n").unwrap_or((got, ""));
        Answer {
            status: common::status(got),
            head: head.to_string(),
            body: body.to_string(),
        }
    }
}

/// Requests with their headers as pairs, answered as an `Answer`.
impl Server {
    fn ask(&self, method: &str, target: &str, headers: &[(&str, &str)], body: &[u8]) -> Answer {
        self.ask_declaring(method, target, headers, body.len(), body)
    }

    /// Says the body is `declared` long, whatever `body` is (see `request_declaring`).
    fn ask_declaring(
        &self,
        method: &str,
        target: &str,
        headers: &[(&str, &str)],
        declared: usize,
        body: &[u8],
    ) -> Answer {
        let headers: String = headers
            .iter()
            .map(|(n, v)| format!("{n}: {v}\r\n"))
            .collect();
        Answer::of(&self.request_declaring(method, target, &headers, declared, body))
    }

    fn get(&self, target: &str, headers: &[(&str, &str)]) -> Answer {
        self.ask("GET", target, headers, b"")
    }
}

const FORM: (&str, &str) = ("content-type", "application/x-www-form-urlencoded");

#[test]
fn the_body_limit_is_the_operators() {
    let form = |len: usize| format!("name={}", "a".repeat(len)).into_bytes();
    let post =
        |server: &Server, len: usize| server.ask("POST", "/login", &[FORM], &form(len)).status;
    // Refused by its announced length alone, before a byte of it is sent.
    let refused = |server: &Server, len: usize| {
        server
            .ask_declaring("POST", "/login", &[FORM], form(len).len(), b"")
            .status
    };
    let default = start(&[]);
    assert_eq!(post(&default, 2000), 303, "1 MB by default");
    let small = start(&[("WISP_BODY_LIMIT", "1KB")]);
    assert_eq!(refused(&small, 2000), 413);
    assert_eq!(post(&small, 100), 303);
    // Any of the spellings: bytes, KB, MB, with a space or in lower case.
    for (limit, over) in [("1500", 1600), ("2kb", 2100), (" 1 KB ", 1100), ("1B", 10)] {
        let s = start(&[("WISP_BODY_LIMIT", limit)]);
        assert_eq!(refused(&s, over), 413, "{limit}");
    }
    let big = start(&[("WISP_BODY_LIMIT", "2MB")]);
    assert_eq!(post(&big, 1_500_000), 303);
    // A route's own `BODY_LIMIT` is its own: `/echo` takes 64 KB whatever the app's limit is.
    assert_eq!(
        small.ask("POST", "/echo", &[], &vec![b'x'; 60_000]).status,
        200
    );
    assert_eq!(
        big.ask_declaring("POST", "/echo", &[], 70_000, b"").status,
        413
    );
}

#[test]
fn the_client_address_is_the_peers_unless_a_proxy_is_trusted() {
    let proxied = ("x-forwarded-for", "6.6.6.6, 1.2.3.4");
    let plain = start(&[]);
    // A header any client can send is never trusted by default.
    assert_eq!(plain.get("/t/ip", &[proxied]).body, "127.0.0.1|127.0.0.1");

    let trusting = start(&[("WISP_CLIENT_IP_HEADER", "X-Forwarded-For")]);
    // The entry the proxy added is the last.
    assert_eq!(trusting.get("/t/ip", &[proxied]).body, "127.0.0.1|1.2.3.4");
    assert_eq!(
        trusting
            .get("/t/ip", &[("x-forwarded-for", "2001:db8::1")])
            .body,
        "127.0.0.1|2001:db8::1"
    );
    // Not an address: the peer's.
    assert_eq!(
        trusting.get("/t/ip", &[("x-forwarded-for", "nope")]).body,
        "127.0.0.1|127.0.0.1"
    );
    assert_eq!(trusting.get("/t/ip", &[]).body, "127.0.0.1|127.0.0.1");
    // Another header stays ignored.
    assert_eq!(
        trusting.get("/t/ip", &[("x-real-ip", "1.2.3.4")]).body,
        "127.0.0.1|127.0.0.1"
    );
}

#[test]
fn a_request_id_is_made_for_every_request_when_asked_to() {
    let default = start(&[]);
    assert_eq!(default.get("/", &[]).header("x-request-id"), None);
    let id = default.get("/t/id", &[]);
    assert_eq!(
        id.header("x-request-id"),
        Some(id.body.as_str()),
        "asked for by the route"
    );

    let every = start(&[("WISP_REQUEST_ID", "on")]);
    let home = every.get("/", &[]);
    assert_eq!(home.header("x-request-id").map(str::len), Some(16));
    let sent = every.get("/nope", &[("x-request-id", "trace-42")]);
    assert_eq!(
        (sent.status, sent.header("x-request-id")),
        (404, Some("trace-42"))
    );
    for value in ["1", "true", "ON"] {
        let s = start(&[("WISP_REQUEST_ID", value)]);
        assert!(s.get("/", &[]).header("x-request-id").is_some(), "{value}");
    }
    for value in ["off", "0", "false"] {
        let s = start(&[("WISP_REQUEST_ID", value)]);
        assert!(s.get("/", &[]).header("x-request-id").is_none(), "{value}");
    }
}

#[test]
fn the_api_docs_are_on_in_dev_builds_and_can_be_switched() {
    let default = start(&[]);
    let spec = default.get("/_wisp/openapi.json", &[]);
    assert_eq!(spec.status, 200);
    assert_eq!(spec.header("content-type"), Some("application/json"));
    assert!(spec.body.contains("\"openapi\""), "{}", spec.body);
    assert!(spec.body.contains("/echo"));
    let docs = default.get("/_wisp/docs", &[]);
    assert_eq!(
        (docs.status, docs.header("content-type")),
        (200, Some("text/html; charset=utf-8"))
    );
    assert!(
        docs.body.contains("<html") || docs.body.contains("<!doctype"),
        "{}",
        docs.body
    );
    // Only GET and HEAD read them.
    assert_ne!(default.ask("POST", "/_wisp/docs", &[], b"").status, 200);

    for off in ["off", "0", "false"] {
        let hidden = start(&[("WISP_API_DOCS", off)]);
        assert_eq!(hidden.get("/_wisp/openapi.json", &[]).status, 404, "{off}");
        assert_eq!(hidden.get("/_wisp/docs", &[]).status, 404, "{off}");
    }
    let on = start(&[("WISP_API_DOCS", "on")]);
    assert_eq!(on.get("/_wisp/docs", &[]).status, 200);
}

#[test]
fn origin_names_the_site_for_forms_and_secure_cookies() {
    let same_site = start(&[("ORIGIN", "https://example.com/")]);
    let login = |s: &Server, origin: &str| {
        s.ask("POST", "/login", &[FORM, ("origin", origin)], b"name=ada")
    };
    // The site's own address, whatever `Host` the proxy passed on.
    let ok = login(&same_site, "https://example.com");
    assert_eq!(ok.status, 303);
    assert!(
        ok.head
            .lines()
            .any(|l| l.to_ascii_lowercase().starts_with("set-cookie: user=")
                && l.contains("; Secure")),
        "cookies are Secure when the site is HTTPS: {}",
        ok.head
    );
    assert_eq!(login(&same_site, "https://EXAMPLE.com").status, 303);
    // Not the address it is reached at: that would let any site that guesses it in.
    assert_eq!(
        login(&same_site, &format!("http://127.0.0.1:{}", same_site.port)).status,
        403
    );
    assert_eq!(login(&same_site, "https://evil.example").status, 403);

    let open = start(&[]);
    let plain = login(&open, &format!("http://127.0.0.1:{}", open.port));
    assert_eq!(plain.status, 303);
    assert!(!plain.head.contains("Secure"), "{}", plain.head);
    assert_eq!(login(&open, "https://example.com").status, 403);
    // Behind a proxy that says it spoke HTTPS.
    let proxied = open.ask(
        "POST",
        "/login",
        &[FORM, ("x-forwarded-proto", "https")],
        b"name=ada",
    );
    assert!(proxied.head.contains("; Secure"), "{}", proxied.head);
}

#[test]
fn threads_and_hosts_are_taken_as_given() {
    for threads in ["1", "3"] {
        let s = start(&[("WISP_THREADS", threads)]);
        assert_eq!(s.get("/", &[]).status, 200, "{threads} threads");
    }
    // Any number of connections may be allowed, or none capped.
    for max in ["50", "0"] {
        let s = start(&[("WISP_MAX_CONNS", max)]);
        assert_eq!(s.get("/", &[]).status, 200, "{max}");
    }
}

#[test]
fn under_wisp_dev_the_app_leaves_with_its_parent() {
    // `wisp dev` holds the app's stdin: when it goes, however it went, so does the app.
    let mut child = command(&[("WISP_DEV_EVENTS", "1")])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert!(line.contains("listening on"), "{line}");
    assert!(
        child.try_wait().unwrap().is_none(),
        "still serving while its parent holds the pipe"
    );
    drop(child.stdin.take());
    assert_eq!(exits(&mut child).code(), Some(0));
}

#[test]
fn export_mode_writes_the_site_instead_of_serving_it() {
    let dir = Temp::new("export");
    let (code, out) = stops(&[("WISP_EXPORT", dir.to_str().unwrap())]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("wrote index.html"), "{out}");
    assert!(out.contains("wrote 404.html"), "{out}");
    // What cannot be static is said, and left out.
    assert!(out.contains("warn /echo has a +server.rs"), "{out}");
    assert!(!out.contains("listening"), "{out}");
    let home = std::fs::read_to_string(dir.join("index.html")).unwrap();
    assert!(home.contains("<h1>hello from init</h1>"), "{home}");
    assert!(dir.join("_app/wisp.js").is_file());
    assert!(!dir.join("echo").exists());

    // Somewhere that cannot be written to: a failure that names it, not a panic.
    let file = Temp::new("not-a-folder");
    std::fs::write(&*file, "x").unwrap();
    let (code, out) = stops(&[("WISP_EXPORT", file.to_str().unwrap())]);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains("wisp:"), "{out}");
}

#[test]
fn a_session_lasts_as_long_as_the_secret_does() {
    // Signed in on one server: the cookie it set.
    let first = start(&[]);
    let login = first.ask("POST", "/login", &[FORM], b"name=ada");
    assert_eq!(login.status, 303);
    let cookie = login
        .head
        .lines()
        .find_map(|l| l.strip_prefix("set-cookie: "))
        .and_then(|c| c.split(';').next())
        .unwrap()
        .to_string();
    assert!(cookie.starts_with("user=ada."), "{cookie}");
    let admin = |s: &Server, cookie: &str| s.get("/admin", &[("cookie", cookie)]).status;
    assert_eq!(admin(&first, &cookie), 200);
    // A restart with the same secret keeps everyone signed in.
    let same = start(&[]);
    assert_eq!(admin(&same, &cookie), 200);
    // Another secret is another site: nothing signed by the first is believed, and the
    // visitor is sent to sign in, not shown an error.
    let other = start(&[("WISP_SECRET", "another-secret-of-at-least-32-characters")]);
    let refused = other.get("/admin", &[("cookie", &cookie)]);
    assert_eq!(
        (refused.status, refused.header("location")),
        (303, Some("/login"))
    );
}

#[test]
fn rows_outlive_a_killed_server_and_a_write_it_died_in() {
    let dir = Temp::new("data");
    let data = dir.to_str().unwrap();
    let auth = ("authorization", "Bearer wisp-test-app");
    let json = ("content-type", "application/json");

    let first = start(&[("WISP_DATA", data)]);
    let made = first.ask("POST", "/notes", &[auth, json], br#"{"title":"Tea"}"#);
    assert_eq!(made.status, 201, "{}", made.head);
    // Killed, not stopped: what was answered is already in the log.
    drop(first);
    let log = dir.join("note.log");
    assert!(std::fs::read_to_string(&log).unwrap().starts_with("1\t{"));

    // The end of a write the process died in, as a crash would leave it.
    let mut file = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
    file.write_all(b"2\t{\"title\":\"Half").unwrap();
    drop(file);

    let second = start(&[("WISP_DATA", data)]);
    let list = second.get("/notes", &[]).body;
    assert!(list.contains("Tea") && !list.contains("Half"), "{list}");
    assert!(
        !std::fs::read_to_string(&log).unwrap().contains("Half"),
        "the log is made whole"
    );
    // Ids are not given twice, and the next row lands after the good ones.
    let next = second.ask("POST", "/notes", &[auth, json], br#"{"title":"Milk"}"#);
    assert_eq!(next.status, 201);
    assert!(next.body.contains("\"id\":2"), "{}", next.body);
    drop(second);
    let third = start(&[("WISP_DATA", data)]);
    let list = third.get("/notes", &[]).body;
    assert!(list.contains("Tea") && list.contains("Milk"), "{list}");
}

#[test]
fn an_address_the_machine_does_not_have_says_so() {
    // 192.0.2.1 is TEST-NET-1: reserved for documentation, on no interface. (A Linux host that
    // lets programs bind addresses it does not have, as some proxies' do, has no error to show.)
    if std::fs::read_to_string("/proc/sys/net/ipv4/ip_nonlocal_bind").is_ok_and(|s| s.trim() == "1")
    {
        return;
    }
    let (code, out) = stops(&[("HOST", "192.0.2.1")]);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains("cannot listen on 192.0.2.1"), "{out}");
    assert!(out.contains("This machine has no such address"), "{out}");
}
