//! What the test app's binary tells an operator: `WISP_LOG=json` lines.

mod common;

use common::{Server, body, command, header, start, status};
use std::io::{BufRead, BufReader};
use std::process::{ChildStdout, Stdio};
use wisp::Value;

/// The app with `env`, and its stdout after the line with its port.
fn start_reading(env: &[(&str, &str)]) -> (Server, BufReader<ChildStdout>) {
    let mut child = command(env)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("start the test app");
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    let port = line.trim().rsplit(':').next().unwrap().parse().unwrap();
    (Server { child, port }, out)
}

fn next_json(out: &mut BufReader<ChildStdout>) -> Value {
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    wisp::json::parse(&line).unwrap_or_else(|e| panic!("{e}: {line:?}"))
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{key} in {v:?}"))
}

fn number(v: &Value, key: &str) -> i64 {
    v.get(key)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("{key} in {v:?}"))
}

#[test]
fn metrics_need_the_key_and_count_each_route() {
    let s = start(&[("METRICS_KEY", "sesame-123"), ("WISP_DEV", "off")]);
    for path in ["/post/a", "/post/b", "/no/such/page"] {
        s.request("GET", path, "", b"");
    }
    let open = s.request("GET", "/_wisp/metrics", "", b"");
    assert_eq!(status(&open), 401, "{open}");
    assert_eq!(header(&open, "www-authenticate"), Some("Bearer"));
    let wrong = s.request(
        "GET",
        "/_wisp/metrics",
        "authorization: Bearer nope\r\n",
        b"",
    );
    assert_eq!(status(&wrong), 401);

    let auth = "authorization: Bearer sesame-123\r\n";
    let page = s.request("GET", "/_wisp/metrics", auth, b"");
    assert_eq!(status(&page), 200, "{page}");
    assert!(
        header(&page, "content-type")
            .unwrap()
            .starts_with("text/plain; version=0.0.4")
    );
    let text = body(&page);
    for want in [
        "wisp_requests_total{route=\"/post/[slug]\",status=\"2xx\"} 2\n",
        "wisp_requests_total{route=\"\",status=\"4xx\"} 3\n", // the 404 and the 401s
        "wisp_request_duration_seconds_count{route=\"/post/[slug]\"} 2\n",
        "wisp_request_duration_seconds_bucket{route=\"/post/[slug]\",le=\"+Inf\"} 2\n",
        "wisp_requests_in_flight 1\n", // this one
        "wisp_uptime_seconds ",
    ] {
        assert!(text.contains(want), "{want}\n{text}");
    }
    if cfg!(target_os = "linux") {
        assert!(text.contains("process_resident_memory_bytes "), "{text}");
    }

    // Without the key there is nothing there.
    let off = start(&[("WISP_DEV", "off")]);
    assert_eq!(
        status(&off.request("GET", "/_wisp/metrics", auth, b"")),
        404
    );
}

#[test]
fn json_logs_a_line_a_request() {
    let (s, mut out) = start_reading(&[("WISP_LOG", "json"), ("WISP_DEV", "off")]);
    let page = s.request("GET", "/post/hello?x=1", "x-request-id: req-7\r\n", b"");
    assert_eq!(status(&page), 200, "{page}");
    let v = next_json(&mut out);
    assert_eq!(text(&v, "method"), "GET");
    assert_eq!(text(&v, "route"), "/post/[slug]");
    assert_eq!(
        text(&v, "path"),
        "/post/hello",
        "no query: it may hold secrets"
    );
    assert_eq!(number(&v, "status"), 200);
    let len: i64 = header(&page, "content-length").unwrap().parse().unwrap();
    assert_eq!(number(&v, "bytes"), len);
    assert_eq!(text(&v, "id"), "req-7");
    assert_eq!(text(&v, "ip"), "127.0.0.1");
    assert!(text(&v, "time").ends_with('Z'));
    assert!(
        v.get("ms")
            .and_then(Value::as_f64)
            .is_some_and(|ms| ms >= 0.0)
    );

    // Every request gets an id, sent back to correlate with the line.
    let missing = s.request("GET", "/no/such/page", "", b"");
    assert_eq!(status(&missing), 404);
    let v = next_json(&mut out);
    assert!(v.get("route").unwrap().is_null(), "{v:?}");
    assert_eq!(number(&v, "status"), 404);
    assert_eq!(Some(text(&v, "id")), header(&missing, "x-request-id"));
}
