//! What the test app's binary tells an operator: `WISP_LOG=json` lines,
//! `/_wisp/metrics`, and OTLP traces to a fake collector.

mod common;

use common::{Server, body, command, header, start, status};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{ChildStdout, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;
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

/// A fake OTLP collector: each POST's body, sent on as it comes, each
/// answered 200.
fn collector() -> (String, Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for c in listener.incoming() {
            let mut c = BufReader::new(c.unwrap());
            let mut len = 0;
            loop {
                let mut line = String::new();
                c.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some(n) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = n.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; len];
            c.read_exact(&mut body).unwrap();
            let _ = c
                .get_mut()
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}");
            if tx.send(String::from_utf8(body).unwrap()).is_err() {
                return;
            }
        }
    });
    (url, rx)
}

/// Every span the collector gets within a few seconds, until `enough`.
fn spans(rx: &Receiver<String>, enough: usize) -> Vec<Value> {
    let mut all = Vec::new();
    while all.len() < enough {
        let body = rx.recv_timeout(Duration::from_secs(10)).expect("spans");
        let v = wisp::json::parse(&body).unwrap();
        let rs = &v.get("resourceSpans").unwrap().as_array().unwrap()[0];
        let name = rs.get("resource").unwrap().get("attributes").unwrap();
        assert!(format!("{name:?}").contains("test-app"), "{body}");
        let scope = &rs.get("scopeSpans").unwrap().as_array().unwrap()[0];
        all.extend(
            scope
                .get("spans")
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .cloned(),
        );
    }
    all
}

#[test]
fn traces_go_to_the_collector_and_continue_the_callers() {
    let (url, rx) = collector();
    let s = start(&[
        ("OTEL_EXPORTER_OTLP_ENDPOINT", url.as_str()),
        ("OTEL_BSP_SCHEDULE_DELAY", "50"),
        ("OTEL_SERVICE_NAME", "test-app"),
        ("WISP_DEV", "off"),
    ]);
    let caller = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    let page = s.request("GET", "/traced", &format!("traceparent: {caller}\r\n"), b"");
    assert_eq!(status(&page), 200, "{page}");
    let ours = header(&page, "traceparent").expect("the reply's traceparent");
    let (inner, ours) = (body(&page), ours.to_string());
    assert_eq!(ours[..36], caller[..36], "the same trace");
    assert_eq!(
        (inner[..36].to_string(), &inner[52..]),
        (ours[..36].to_string(), "-01")
    );
    assert!(ours[36..52] != caller[36..52] && inner[36..52] != ours[36..52]);

    let all = spans(&rx, 2);
    let field = |s: &Value, k: &str| s.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let server = all
        .iter()
        .find(|s| field(s, "name") == "GET /traced")
        .expect("{all:?}");
    assert_eq!(field(server, "traceId"), &caller[3..35]);
    assert_eq!(field(server, "parentSpanId"), &caller[36..52]);
    assert_eq!(field(server, "spanId"), &ours[36..52]);
    assert_eq!(server.get("kind").and_then(Value::as_i64), Some(2));
    let attrs = format!("{:?}", server.get("attributes").unwrap());
    for want in ["http.route", "/traced", "http.response.status_code", "200"] {
        assert!(attrs.contains(want), "{want}: {attrs}");
    }
    let work = all
        .iter()
        .find(|s| field(s, "name") == "work")
        .expect("{all:?}");
    assert_eq!(field(work, "traceId"), &caller[3..35]);
    assert_eq!(field(work, "parentSpanId"), &ours[36..52]);
    assert_eq!(field(work, "spanId"), &inner[36..52]);

    // A request with none starts a trace of its own.
    let fresh = s.request("GET", "/post/x", "", b"");
    let theirs = header(&fresh, "traceparent").unwrap().to_string();
    let server = spans(&rx, 1)
        .into_iter()
        .find(|s| field(s, "name") == "GET /post/[slug]");
    let server = server.expect("its span");
    assert_eq!(field(&server, "traceId"), &theirs[3..35]);
    assert!(server.get("parentSpanId").is_none());
}

/// A collector that is down costs requests nothing.
#[test]
fn traces_to_nowhere_leave_requests_alone() {
    let gone = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", gone.local_addr().unwrap());
    drop(gone);
    let s = start(&[
        ("OTEL_EXPORTER_OTLP_ENDPOINT", url.as_str()),
        ("OTEL_BSP_SCHEDULE_DELAY", "10"),
        ("WISP_DEV", "off"),
    ]);
    for _ in 0..20 {
        assert_eq!(status(&s.request("GET", "/traced", "", b"")), 200);
    }
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
