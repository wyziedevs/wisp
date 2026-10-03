//! Traces to an OpenTelemetry collector: with `OTEL_EXPORTER_OTLP_ENDPOINT`
//! set (`http://localhost:4318`, or `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` for
//! the whole address), each request is one server span, sent as OTLP/HTTP
//! JSON in batches by a thread of its own. `OTEL_SERVICE_NAME` names the
//! service (default `wisp`). A request's `traceparent` is continued.
//!
//! Plain http only, which is what a collector on the same host or network
//! takes; unset, nothing runs and a request pays one load of a flag. A span
//! the collector is too slow to take is dropped, never waited for.

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use real::*;

#[cfg(target_arch = "wasm32")]
pub(crate) fn wanted() -> bool {
    false
}
#[cfg(target_arch = "wasm32")]
pub(crate) fn init() {}
#[cfg(target_arch = "wasm32")]
pub(crate) fn span(_: &crate::cx::Cx, _: u16, _: std::time::Duration) {}

#[cfg(not(target_arch = "wasm32"))]
mod real {
    use crate::cx::Cx;
    use std::fmt::Write as _;
    use std::io::{Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
    use std::sync::{Once, OnceLock};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    static ON: AtomicBool = AtomicBool::new(false);
    static TX: OnceLock<SyncSender<Span>> = OnceLock::new();

    /// Spans waiting for the thread; past this they are dropped.
    const QUEUE: usize = 4096;
    /// Spans in a batch at most, and how long the first waits for company.
    const BATCH: usize = 512;
    const LINGER: Duration = Duration::from_secs(1);

    /// Starts the exporter when the environment names a collector. Cheap to
    /// call again.
    pub(crate) fn init() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let Some(url) = endpoint() else { return };
            let Some(to) = Endpoint::parse(&url) else {
                crate::http::log(format_args!(
                    "wisp: OTEL_EXPORTER_OTLP_ENDPOINT is {url:?}: only http://host:port is supported, so no traces are sent"
                ));
                return;
            };
            let service = crate::setting::<String>("OTEL_SERVICE_NAME", "a name")
                .unwrap_or_else(|| "wisp".into());
            let (tx, rx) = sync_channel(QUEUE);
            let spawned = std::thread::Builder::new()
                .name("wisp-otel".into())
                .spawn(move || export(rx, &to, &service));
            if spawned.is_ok() && TX.set(tx).is_ok() {
                ON.store(true, Ordering::Relaxed);
            }
        });
    }

    /// Whether the environment names a collector, which is what `init`
    /// starts the exporter for.
    pub(crate) fn wanted() -> bool {
        endpoint().is_some()
    }

    fn endpoint() -> Option<String> {
        if let Some(all) = crate::setting::<String>("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", "a URL") {
            return Some(all);
        }
        let base = crate::setting::<String>("OTEL_EXPORTER_OTLP_ENDPOINT", "a URL")?;
        Some(format!("{}/v1/traces", base.trim_end_matches('/')))
    }

    /// One finished request.
    struct Span {
        trace: [u8; 16],
        id: [u8; 8],
        parent: Option<[u8; 8]>,
        start: u64,
        end: u64,
        method: &'static str,
        path: String,
        status: u16,
    }

    /// Queues the span of the request in `cx`, answered with `status` after `took`.
    pub(crate) fn span(cx: &Cx, status: u16, took: Duration) {
        let Some(tx) = TX.get() else { return };
        let end = nanos();
        let (trace, parent) = match cx.header("traceparent").and_then(traceparent) {
            Some((t, p)) => (t, Some(p)),
            None => (ids(), None),
        };
        let _ = tx.try_send(Span {
            trace,
            id: ids()[..8].try_into().unwrap_or_default(),
            parent,
            start: end.saturating_sub(took.as_nanos() as u64),
            end,
            method: cx.method.as_str(),
            path: cx.path().to_owned(),
            status,
        });
    }

    fn nanos() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64)
    }

    /// 16 random-looking bytes: ids need to differ, not to be secret.
    fn ids() -> [u8; 16] {
        static N: AtomicU64 = AtomicU64::new(0);
        let mut seed = N.fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed) ^ nanos();
        let mut out = [0; 16];
        for half in out.chunks_mut(8) {
            seed = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = seed;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            half.copy_from_slice(&(z ^ (z >> 31)).to_le_bytes());
        }
        out
    }

    /// The trace and parent span of a W3C `traceparent`:
    /// `00-<32 hex>-<16 hex>-<flags>`.
    fn traceparent(v: &str) -> Option<([u8; 16], [u8; 8])> {
        let mut p = v.trim().split('-');
        let (_, trace, parent) = (p.next()?, p.next()?, p.next()?);
        let (trace, parent) = (unhex::<16>(trace)?, unhex::<8>(parent)?);
        (trace != [0; 16] && parent != [0; 8]).then_some((trace, parent))
    }

    fn unhex<const N: usize>(s: &str) -> Option<[u8; N]> {
        if s.len() != N * 2 || !s.is_ascii() {
            return None;
        }
        let mut out = [0; N];
        for (i, b) in out.iter_mut().enumerate() {
            *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
        }
        Some(out)
    }

    fn hex(out: &mut String, bytes: &[u8]) {
        for b in bytes {
            let _ = write!(out, "{b:02x}");
        }
    }

    fn quoted(out: &mut String, s: &str) {
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                c if (c as u32) < 0x20 => {
                    let _ = write!(out, "\\u{:04x}", c as u32);
                }
                c => out.push(c),
            }
        }
        out.push('"');
    }

    /// `spans` as an OTLP/HTTP JSON request body.
    fn json(spans: &[Span], service: &str) -> String {
        let mut o = String::from(
            r#"{"resourceSpans":[{"resource":{"attributes":[{"key":"service.name","value":{"stringValue":"#,
        );
        quoted(&mut o, service);
        o.push_str(r#"}}]},"scopeSpans":[{"scope":{"name":"wisp"},"spans":["#);
        for (i, s) in spans.iter().enumerate() {
            if i > 0 {
                o.push(',');
            }
            o.push_str(r#"{"traceId":""#);
            hex(&mut o, &s.trace);
            o.push_str(r#"","spanId":""#);
            hex(&mut o, &s.id);
            if let Some(p) = &s.parent {
                o.push_str(r#"","parentSpanId":""#);
                hex(&mut o, p);
            }
            o.push_str(r#"","name":"#);
            quoted(&mut o, &format!("{} {}", s.method, s.path));
            let _ = write!(
                o,
                r#","kind":2,"startTimeUnixNano":"{}","endTimeUnixNano":"{}","attributes":[{{"key":"http.request.method","value":{{"stringValue":"{}"}}}},{{"key":"url.path","value":{{"stringValue":"#,
                s.start, s.end, s.method
            );
            quoted(&mut o, &s.path);
            let _ = write!(
                o,
                r#"}}}},{{"key":"http.response.status_code","value":{{"intValue":"{}"}}}}]"#,
                s.status
            );
            if s.status >= 500 {
                o.push_str(r#","status":{"code":2}"#);
            }
            o.push('}');
        }
        o.push_str("]}]}]}");
        o
    }

    /// Where the collector is.
    struct Endpoint {
        host: String,
        port: u16,
        path: String,
    }

    impl Endpoint {
        fn parse(url: &str) -> Option<Endpoint> {
            let rest = url.strip_prefix("http://")?;
            let (hostport, path) = rest.split_once('/').map_or((rest, ""), |(h, p)| (h, p));
            let (host, port) = match hostport.rsplit_once(':') {
                Some((h, p)) => (h, p.parse().ok()?),
                None => (hostport, 80),
            };
            (!host.is_empty()).then(|| Endpoint {
                host: host.to_owned(),
                port,
                path: format!("/{path}"),
            })
        }
    }

    /// Sends what comes, in batches, until the server ends.
    fn export(rx: Receiver<Span>, to: &Endpoint, service: &str) {
        let mut failed = false;
        while let Ok(first) = rx.recv() {
            let mut batch = vec![first];
            let by = Instant::now() + LINGER;
            while batch.len() < BATCH {
                match rx.try_recv() {
                    Ok(s) => batch.push(s),
                    Err(TryRecvError::Disconnected) => break,
                    Err(TryRecvError::Empty) => match by.checked_duration_since(Instant::now()) {
                        Some(left) => std::thread::sleep(left.min(Duration::from_millis(50))),
                        None => break,
                    },
                }
            }
            match post(to, &json(&batch, service)) {
                // Said once, not for every batch of a collector that is down.
                Err(e) if !failed => {
                    failed = true;
                    crate::http::log(format_args!(
                        "wisp: traces to {}:{} failed: {e} (spans are dropped until it is back)",
                        to.host, to.port
                    ));
                }
                Ok(()) => failed = false,
                Err(_) => {}
            }
        }
    }

    fn post(to: &Endpoint, body: &str) -> std::io::Result<()> {
        let timeout = Duration::from_secs(2);
        let addr = (to.host.as_str(), to.port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| std::io::Error::other("no address"))?;
        let mut s = TcpStream::connect_timeout(&addr, timeout)?;
        s.set_write_timeout(Some(timeout))?;
        s.set_read_timeout(Some(timeout))?;
        write!(
            s,
            "POST {} HTTP/1.1\r\nhost: {}:{}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            to.path,
            to.host,
            to.port,
            body.len()
        )?;
        let mut head = [0; 12];
        s.read_exact(&mut head)?;
        match &head[9..12] {
            b"200" | b"202" => Ok(()),
            code => Err(std::io::Error::other(format!(
                "the collector answered {}",
                String::from_utf8_lossy(code)
            ))),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::net::TcpListener;

        #[test]
        fn endpoints() {
            let e = Endpoint::parse("http://localhost:4318/v1/traces").unwrap();
            assert_eq!(
                (e.host.as_str(), e.port, e.path.as_str()),
                ("localhost", 4318, "/v1/traces")
            );
            let e = Endpoint::parse("http://otel").unwrap();
            assert_eq!((e.port, e.path.as_str()), (80, "/"));
            assert!(Endpoint::parse("https://otel:4318").is_none());
            assert!(Endpoint::parse("http://:4318").is_none());
            assert!(Endpoint::parse("http://h:port").is_none());
        }

        #[test]
        fn traceparents() {
            let (t, p) =
                traceparent("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01").unwrap();
            assert_eq!(t[0], 0x4b);
            assert_eq!(p[7], 0xb7);
            assert!(traceparent("00-short-00f067aa0ba902b7-01").is_none());
            assert!(
                traceparent("00-00000000000000000000000000000000-00f067aa0ba902b7-01").is_none()
            );
            assert!(traceparent("nonsense").is_none());
        }

        #[test]
        fn ids_differ() {
            assert_ne!(ids(), ids());
        }

        fn span(status: u16) -> Span {
            Span {
                trace: [1; 16],
                id: [2; 8],
                parent: Some([3; 8]),
                start: 10,
                end: 20,
                method: "GET",
                path: "/a\"b".into(),
                status,
            }
        }

        #[test]
        fn json_is_otlp() {
            let body = json(&[span(200), span(500)], "shop");
            let v = crate::json::parse(&body).expect("valid JSON");
            let text = format!("{v:?}");
            for want in [
                "resourceSpans",
                "scopeSpans",
                "shop",
                "01010101010101010101010101010101",
                "0303030303030303",
                "GET /a\\\"b",
            ] {
                assert!(text.contains(want), "{want} in {text}");
            }
            assert_eq!(body.matches(r#""status":{"code":2}"#).count(), 1);
            assert!(body.contains(r#""startTimeUnixNano":"10""#));
        }

        #[test]
        fn a_batch_reaches_the_collector() {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = l.local_addr().unwrap().port();
            let t = std::thread::spawn(move || {
                let (mut c, _) = l.accept().unwrap();
                let mut got = Vec::new();
                let mut buf = [0; 4096];
                while !got.windows(4).any(|w| w == b"\r\n\r\n") || got.len() < 400 {
                    let n = c.read(&mut buf).unwrap();
                    if n == 0 {
                        break;
                    }
                    got.extend_from_slice(&buf[..n]);
                }
                c.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}")
                    .unwrap();
                String::from_utf8(got).unwrap()
            });
            let to = Endpoint::parse(&format!("http://127.0.0.1:{port}/v1/traces")).unwrap();
            post(&to, &json(&[span(200)], "t")).unwrap();
            let got = t.join().unwrap();
            assert!(got.starts_with("POST /v1/traces HTTP/1.1\r\n"));
            assert!(got.contains("content-type: application/json"));
            assert!(got.contains(r#""traceId":"01010101"#));
        }

        #[test]
        fn a_collector_that_is_down_is_an_error_not_a_hang() {
            let port = {
                let l = TcpListener::bind("127.0.0.1:0").unwrap();
                l.local_addr().unwrap().port()
            };
            let to = Endpoint::parse(&format!("http://127.0.0.1:{port}/")).unwrap();
            assert!(post(&to, "{}").is_err());
        }
    }
}
