//! OpenTelemetry traces, when `OTEL_EXPORTER_OTLP_ENDPOINT` is set: a span
//! a request, and one for each [`span`] inside it, sent as OTLP/HTTP JSON
//! by a thread of their own, in batches. The request's `traceparent` is
//! continued, and the reply carries this one's.
//!
//! OTLP's JSON is a fixed shape, so it is written here, as is the POST:
//! no opentelemetry crates. An exporter that fails costs requests nothing:
//! spans wait in a bounded queue and past it are dropped, and the thread
//! says so once a minute.

use crate::cx::valid_header;
use std::cell::Cell;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Condvar, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime};

/// Spans waiting at most; more are dropped.
const QUEUE: usize = 2048;
/// Spans a POST; a full batch is sent at once.
const BATCH: usize = 512;
/// Longest a connect, a write or a read to the collector takes.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Where spans go, and those on the way.
pub(crate) struct Otel {
    /// `host:port`, as the `Host` header names it.
    host: String,
    /// What to connect to: `host` with a port.
    addr: String,
    path: String,
    /// `OTEL_EXPORTER_OTLP_HEADERS`, as header lines.
    headers: String,
    service: String,
    /// `OTEL_BSP_SCHEDULE_DELAY`: longest a span waits to be sent.
    delay: Duration,
    queue: Mutex<Vec<Span>>,
    wake: Condvar,
    dropped: AtomicU64,
    /// An instant and the wall clock then, in ns since 1970: spans are
    /// timed by the monotonic clock and sent by the wall's.
    clock: (Instant, u64),
}

/// A span's place in its trace: what `traceparent` carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Ctx {
    /// High and low halves: a `u128` would align every future that holds
    /// one to 16 bytes.
    pub trace: [u64; 2],
    pub span: u64,
    pub sampled: bool,
}

/// A span ended, waiting to be sent.
pub(crate) struct Span {
    pub ctx: Ctx,
    pub parent: Option<u64>,
    /// A user span's name; the request's is made from `server`.
    pub name: &'static str,
    pub start: Instant,
    pub end: Instant,
    pub server: Option<Server>,
}

/// What the request's span says of it.
pub(crate) struct Server {
    pub method: &'static str,
    pub route: Option<&'static str>,
    pub path: String,
    pub status: u16,
}

thread_local! {
    /// The span of what runs now: the request's while its handler is
    /// polled, or a [`span`] inside it.
    static CURRENT: Cell<Option<Ctx>> = const { Cell::new(None) };
    /// A request's span on its way to its handler.
    static NEXT: Cell<Option<Ctx>> = const { Cell::new(None) };
}

/// Hands `c` to the handler made next on this thread (see [`adopt`]):
/// its span, made current each time it is polled, as it shares its
/// thread with other requests between polls.
pub(crate) fn hand(c: Ctx) {
    HANDED.store(true, Relaxed);
    NEXT.set(Some(c));
}

/// What [`hand`] gave, once. Until something is handed (with traces off,
/// always) it is one load.
#[inline]
pub(crate) fn adopt() -> Option<Ctx> {
    match HANDED.load(Relaxed) {
        true => NEXT.take(),
        false => None,
    }
}

pub(crate) fn enter(c: Ctx) -> Option<Ctx> {
    CURRENT.replace(Some(c))
}

pub(crate) fn leave(was: Option<Ctx>) {
    CURRENT.set(was);
}

/// Whether [`hand`] ever ran.
static HANDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A span of the request's, timed until it is dropped: `let _s =
/// wisp::span("charge card");`. Inside a request while traces are on;
/// otherwise it costs a thread-local read and is never sent.
pub fn span(name: &'static str) -> SpanGuard {
    let open = CURRENT.get().map(|parent| {
        let own = Ctx {
            span: id(),
            ..parent
        };
        CURRENT.set(Some(own));
        (own, parent.span, Instant::now())
    });
    SpanGuard { open, name }
}

/// The `traceparent` of the span that runs now, for a request to another
/// service: `req.header("traceparent", &t)`. `None` with traces off.
pub fn traceparent() -> Option<String> {
    CURRENT.get().map(|c| c.header())
}

/// See [`span`].
#[must_use = "the span ends when this is dropped: `let _s = wisp::span(\"x\");`"]
pub struct SpanGuard {
    open: Option<(Ctx, u64, Instant)>,
    name: &'static str,
}

impl Drop for SpanGuard {
    fn drop(&mut self) {
        let Some((own, parent, start)) = self.open.take() else {
            return;
        };
        if CURRENT.get() == Some(own) {
            CURRENT.set(Some(Ctx {
                span: parent,
                ..own
            }));
        }
        if let (true, Some(o)) = (own.sampled, crate::obs::otel()) {
            o.push(Span {
                ctx: own,
                parent: Some(parent),
                name: self.name,
                start,
                end: Instant::now(),
                server: None,
            });
        }
    }
}

impl Ctx {
    /// A span of the trace `incoming` names (a `traceparent`), or of a new
    /// one; and the span it continues.
    pub(crate) fn begin(incoming: Option<&str>) -> (Ctx, Option<u64>) {
        let span = id();
        match incoming.and_then(parse) {
            Some(p) => (Ctx { span, ..p }, Some(p.span)),
            None => {
                let trace = [id(), id()];
                let sampled = true;
                (
                    Ctx {
                        trace,
                        span,
                        sampled,
                    },
                    None,
                )
            }
        }
    }

    pub(crate) fn header(&self) -> String {
        let [hi, lo] = self.trace;
        let flags = u8::from(self.sampled);
        format!("00-{hi:016x}{lo:016x}-{:016x}-{flags:02x}", self.span)
    }
}

/// A `traceparent` (W3C Trace Context): `00-<trace>-<span>-<flags>`, in
/// lowercase hex. A later version may add fields after these.
fn parse(h: &str) -> Option<Ctx> {
    let h = h.trim();
    let b = h.as_bytes();
    if b.len() < 55 || (b.len() > 55 && (h.starts_with("00") || b[55] != b'-')) {
        return None;
    }
    if b[2] != b'-' || b[35] != b'-' || b[52] != b'-' || h.starts_with("ff") {
        return None;
    }
    let hex = |s: &str| s.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'));
    let (v, trace, span, flags) = (&h[..2], &h[3..35], &h[36..52], &h[53..55]);
    if !(hex(v) && hex(trace) && hex(span) && hex(flags)) {
        return None;
    }
    let half = |h: &str| u64::from_str_radix(h, 16).ok();
    let trace = [half(&trace[..16])?, half(&trace[16..])?];
    if trace == [0, 0] {
        return None;
    }
    let span = u64::from_str_radix(span, 16).ok().filter(|&s| s != 0)?;
    let sampled = u8::from_str_radix(flags, 16).ok()? & 1 == 1;
    Some(Ctx {
        trace,
        span,
        sampled,
    })
}

/// A fresh span id, never 0: a counter scrambled by a bijection (so two
/// in a process never match), from a random start.
fn id() -> u64 {
    static SEED: OnceLock<u64> = OnceLock::new();
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let seed = *SEED.get_or_init(|| u64::from_le_bytes(crate::sign::random()));
    let mut z = seed.wrapping_add(
        COUNT
            .fetch_add(1, Relaxed)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15),
    );
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    (z ^ (z >> 31)).max(1)
}

impl Otel {
    /// The exporter the environment asks for, if any; one it cannot use
    /// stops the server at start, saying why.
    pub(crate) fn from_env() -> Option<Otel> {
        let set =
            |name: &str| crate::setting::<String>(name, "an address").filter(|v| !v.is_empty());
        let (name, url, whole) = match set("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT") {
            Some(u) => ("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", u, true),
            None => (
                "OTEL_EXPORTER_OTLP_ENDPOINT",
                set("OTEL_EXPORTER_OTLP_ENDPOINT")?,
                false,
            ),
        };
        let Some(rest) = url.strip_prefix("http://") else {
            crate::fail(&format!(
                "{name} is {url:?}, and Wisp sends OTLP over plain http://\n  \
                 Run an OpenTelemetry Collector beside the app (http://localhost:4318) \
                 and let it forward over TLS."
            ));
        };
        let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
        if host.is_empty() || !valid_header("host", host) || host.contains([' ', '@']) {
            crate::fail(&format!(
                "{name} is {url:?}, which is not an http:// address"
            ));
        }
        let path = match whole {
            true if path.is_empty() => "/".to_string(),
            true => path.to_string(),
            false => format!("{}/v1/traces", path.trim_end_matches('/')),
        };
        let ported = host
            .rsplit_once(':')
            .is_some_and(|(_, p)| p.parse::<u16>().is_ok());
        let addr = if ported {
            host.to_string()
        } else {
            format!("{host}:80")
        };
        let headers = set("OTEL_EXPORTER_OTLP_TRACES_HEADERS")
            .or_else(|| set("OTEL_EXPORTER_OTLP_HEADERS"))
            .map_or_else(String::new, |h| headers(&h));
        let service = set("OTEL_SERVICE_NAME").unwrap_or_else(|| {
            let exe = std::env::current_exe().ok();
            let stem = exe.as_deref().and_then(std::path::Path::file_stem);
            stem.map_or("wisp".into(), |s| s.to_string_lossy().into_owned())
        });
        let delay = crate::setting::<u64>("OTEL_BSP_SCHEDULE_DELAY", "a number of milliseconds")
            .map_or(Duration::from_secs(5), Duration::from_millis);
        Some(Otel {
            host: host.to_string(),
            addr,
            path,
            headers,
            service,
            delay,
            queue: Mutex::new(Vec::with_capacity(BATCH)),
            wake: Condvar::new(),
            dropped: AtomicU64::new(0),
            clock: (Instant::now(), unix_nanos()),
        })
    }

    /// Queues `s`, or drops it when the queue is full.
    pub(crate) fn push(&self, s: Span) {
        let mut q = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        if q.len() >= QUEUE {
            drop(q);
            self.dropped.fetch_add(1, Relaxed);
            return;
        }
        q.push(s);
        if q.len() == BATCH {
            self.wake.notify_one();
        }
    }

    /// The exporter's thread: a batch whenever one is full or the oldest
    /// has waited `delay`, until the process ends.
    pub(crate) fn run(&self) {
        let mut said = None;
        loop {
            let batch = self.take(true);
            if !batch.is_empty() {
                self.ship(&batch, &mut said);
            }
        }
    }

    /// What is queued, sent now: before the process exits.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn flush(&self) {
        let batch = self.take(false);
        if !batch.is_empty() {
            self.ship(&batch, &mut None);
        }
    }

    fn take(&self, wait: bool) -> Vec<Span> {
        let mut q = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        if wait {
            q = (self
                .wake
                .wait_timeout_while(q, self.delay, |q| q.len() < BATCH))
            .unwrap_or_else(PoisonError::into_inner)
            .0;
        }
        std::mem::replace(&mut *q, Vec::with_capacity(BATCH))
    }

    /// Sends `batch`; one that fails is dropped, and said so on stderr at
    /// most once a minute (`said`: when it last was).
    fn ship(&self, batch: &[Span], said: &mut Option<Instant>) {
        let Err(e) = self.post(self.encode(batch).as_bytes()) else {
            return;
        };
        if said.is_some_and(|t| t.elapsed() < Duration::from_secs(60)) {
            return;
        }
        *said = Some(Instant::now());
        let full = self.dropped.swap(0, Relaxed);
        crate::http::log(format_args!(
            "wisp: could not send traces to http://{}{}: {e}; dropped {} span(s){}",
            self.host,
            self.path,
            batch.len(),
            match full {
                0 => String::new(),
                n => format!(", and {n} more the full queue had no room for"),
            }
        ));
    }

    /// One POST of `body`, and the collector's 2xx.
    fn post(&self, body: &[u8]) -> io::Result<()> {
        let at =
            (self.addr.to_socket_addrs()?.next()).ok_or_else(|| io::Error::other("no address"))?;
        let mut s = TcpStream::connect_timeout(&at, TIMEOUT)?;
        s.set_write_timeout(Some(TIMEOUT))?;
        s.set_read_timeout(Some(TIMEOUT))?;
        let head = format!(
            "POST {} HTTP/1.1\r\nhost: {}\r\ncontent-type: application/json\r\n\
             content-length: {}\r\nconnection: close\r\n{}\r\n",
            self.path,
            self.host,
            body.len(),
            self.headers
        );
        s.write_all(head.as_bytes())?;
        s.write_all(body)?;
        let mut got = Vec::new();
        let mut chunk = [0; 512];
        while !got.windows(2).any(|w| w == b"\r\n") && got.len() < 8192 {
            match s.read(&mut chunk)? {
                0 => break,
                n => got.extend_from_slice(&chunk[..n]),
            }
        }
        let line = String::from_utf8_lossy(&got);
        let line = line.lines().next().unwrap_or("");
        match line.split(' ').nth(1).and_then(|c| c.parse::<u16>().ok()) {
            Some(200..=299) => Ok(()),
            Some(_) => Err(io::Error::other(format!("it answered {line:?}"))),
            None => Err(io::Error::other("it did not answer HTTP")),
        }
    }

    /// `batch` as an OTLP `ExportTraceServiceRequest`, in JSON.
    pub(crate) fn encode(&self, batch: &[Span]) -> String {
        use crate::Json;
        use std::fmt::Write;
        let mut s = String::with_capacity(256 + 400 * batch.len());
        s.push_str(r#"{"resourceSpans":[{"resource":{"attributes":["#);
        attr(&mut s, "service.name", &self.service);
        let _ = write!(
            s,
            r#"]}},"scopeSpans":[{{"scope":{{"name":"wisp","version":"{}"}},"spans":["#,
            env!("CARGO_PKG_VERSION")
        );
        for (i, span) in batch.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            let _ = write!(
                s,
                r#"{{"traceId":"{:016x}{:016x}","spanId":"{:016x}","#,
                span.ctx.trace[0], span.ctx.trace[1], span.ctx.span
            );
            if let Some(p) = span.parent {
                let _ = write!(s, r#""parentSpanId":"{p:016x}","#);
            }
            s.push_str(r#""name":"#);
            match &span.server {
                Some(r) => match r.route {
                    Some(route) => format!("{} {route}", r.method).json(&mut s),
                    None => r.method.json(&mut s),
                },
                None => span.name.json(&mut s),
            }
            let kind = if span.server.is_some() { 2 } else { 1 };
            let _ = write!(
                s,
                r#","kind":{kind},"startTimeUnixNano":"{}","endTimeUnixNano":"{}","attributes":["#,
                self.nanos(span.start),
                self.nanos(span.end)
            );
            if let Some(r) = &span.server {
                attr(&mut s, "http.request.method", r.method);
                s.push(',');
                attr(&mut s, "url.path", &r.path);
                if let Some(route) = r.route {
                    s.push(',');
                    attr(&mut s, "http.route", route);
                }
                let _ = write!(
                    s,
                    r#",{{"key":"http.response.status_code","value":{{"intValue":"{}"}}}}"#,
                    r.status
                );
            }
            s.push(']');
            if span.server.as_ref().is_some_and(|r| r.status >= 500) {
                s.push_str(r#","status":{"code":2}"#);
            }
            s.push('}');
        }
        s.push_str("]}]}]}");
        s
    }

    /// `t` in ns since 1970.
    fn nanos(&self, t: Instant) -> u64 {
        let since = t.saturating_duration_since(self.clock.0).as_nanos();
        self.clock
            .1
            .saturating_add(u64::try_from(since).unwrap_or(u64::MAX))
    }
}

/// A string attribute.
fn attr(s: &mut String, key: &str, value: &str) {
    use crate::Json;
    s.push_str(r#"{"key":"#);
    key.json(s);
    s.push_str(r#","value":{"stringValue":"#);
    value.json(s);
    s.push_str("}}");
}

/// `OTEL_EXPORTER_OTLP_HEADERS`, `k=v,k2=v2` with values percent-encoded,
/// as header lines; one that is not a header stops the server.
fn headers(list: &str) -> String {
    let mut out = String::new();
    for pair in list.split(',').filter(|p| !p.trim().is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let (k, v) = (k.trim(), crate::cx::decode(v.trim().as_bytes(), false));
        if !valid_header(k, &v) {
            crate::fail(&format!(
                "OTEL_EXPORTER_OTLP_HEADERS has {k:?}, which is not a header"
            ));
        }
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out
}

pub(crate) fn unix_nanos() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn exporter(addr: &str) -> Otel {
        Otel {
            host: addr.into(),
            addr: addr.into(),
            path: "/v1/traces".into(),
            headers: "x-key: k\r\n".into(),
            service: "shop".into(),
            delay: Duration::from_millis(10),
            queue: Mutex::new(Vec::new()),
            wake: Condvar::new(),
            dropped: AtomicU64::new(0),
            clock: (Instant::now(), 1_000),
        }
    }

    fn server_span(status: u16) -> Span {
        let (ctx, parent) = Ctx::begin(Some(
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        ));
        let start = Instant::now();
        Span {
            ctx,
            parent,
            name: "",
            start,
            end: start + Duration::from_micros(5),
            server: Some(Server {
                method: "GET",
                route: Some("/post/[slug]"),
                path: "/post/\"x\"".into(),
                status,
            }),
        }
    }

    #[test]
    fn traceparents_are_read_and_written() {
        let h = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        let c = parse(h).unwrap();
        assert_eq!(c.trace, [0x4bf92f3577b34da6, 0xa3ce929d0e0e4736]);
        assert_eq!((c.span, c.sampled), (0x00f067aa0ba902b7, true));
        assert_eq!(c.header(), h);
        assert!(!parse(&h.replace("-01", "-00")).unwrap().sampled);
        // A later version may add fields; this one may not.
        assert!(parse(&format!("01{}-what", &h[2..])).is_some());
        for bad in [
            "",
            &h[..54],
            &format!("{h}-x"),
            &h.replace("4bf9", "4BF9"),
            &h.replace("00f067aa0ba902b7", "0000000000000000"),
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
            &format!("ff{}", &h[2..]),
            &h.replace('-', "_"),
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
        let (fresh, parent) = Ctx::begin(Some("nonsense"));
        assert!(parent.is_none() && fresh.sampled && fresh.trace != [0, 0]);
        let (child, parent) = Ctx::begin(Some(h));
        assert_eq!((child.trace, parent), (c.trace, Some(c.span)));
        assert_ne!(child.span, c.span);
        assert_ne!(id(), id());
    }

    #[test]
    fn user_spans_nest_inside_the_current_one() {
        assert!(traceparent().is_none());
        drop(span("outside")); // nothing current: inert
        let (req, _) = Ctx::begin(None);
        let was = enter(req);
        {
            let _a = span("a");
            let a = CURRENT.get().unwrap();
            assert_eq!(a.trace, req.trace);
            assert_ne!(a.span, req.span);
            assert_eq!(traceparent(), Some(a.header()));
        }
        assert_eq!(CURRENT.get(), Some(req));
        leave(was);
        assert!(CURRENT.get().is_none());
    }

    #[test]
    fn spans_encode_as_otlp_json() {
        let o = exporter("127.0.0.1:1");
        let request = server_span(503);
        let mut user = server_span(200);
        (user.server, user.name, user.parent) = (None, "charge", Some(request.ctx.span));
        let text = o.encode(&[request, user]);
        let v = crate::json::parse(&text).unwrap();
        let rs = &v.get("resourceSpans").unwrap().as_array().unwrap()[0];
        let attrs = rs.get("resource").unwrap().get("attributes").unwrap();
        assert!(text.contains(r#""key":"service.name","value":{"stringValue":"shop"}"#));
        assert!(attrs.as_array().is_some());
        let scope = &rs.get("scopeSpans").unwrap().as_array().unwrap()[0];
        let spans = scope.get("spans").unwrap().as_array().unwrap();
        let s = &spans[0];
        let str_of = |s: &crate::Value, k: &str| s.get(k).unwrap().as_str().unwrap().to_string();
        assert_eq!(str_of(s, "traceId"), "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(str_of(s, "parentSpanId"), "00f067aa0ba902b7");
        assert_eq!(str_of(s, "spanId").len(), 16);
        assert_eq!(str_of(s, "name"), "GET /post/[slug]");
        assert_eq!(s.get("kind").unwrap().as_i64(), Some(2));
        let (start, end) = (str_of(s, "startTimeUnixNano"), str_of(s, "endTimeUnixNano"));
        let ns = |t: &str| t.parse::<u64>().unwrap();
        assert_eq!(ns(&end) - ns(&start), 5_000);
        assert!(text.contains(r#"{"key":"url.path","value":{"stringValue":"/post/\"x\""}}"#));
        assert!(text.contains(r#"{"key":"http.route","value":{"stringValue":"/post/[slug]"}}"#));
        assert!(text.contains(r#"{"key":"http.response.status_code","value":{"intValue":"503"}}"#));
        assert_eq!(
            s.get("status").unwrap().get("code").unwrap().as_i64(),
            Some(2)
        );
        let u = &spans[1];
        assert_eq!(
            (str_of(u, "name"), u.get("kind").unwrap().as_i64()),
            ("charge".into(), Some(1))
        );
        assert_eq!(str_of(u, "parentSpanId"), str_of(s, "spanId"));
        assert!(u.get("status").is_none());
    }

    #[test]
    fn the_queue_is_bounded() {
        let o = exporter("127.0.0.1:1");
        for _ in 0..QUEUE + 3 {
            o.push(server_span(200));
        }
        assert_eq!(o.queue.lock().unwrap().len(), QUEUE);
        assert_eq!(o.dropped.load(Relaxed), 3);
        assert_eq!(o.take(false).len(), QUEUE);
        assert!(o.take(false).is_empty());
    }

    /// A batch reaches a collector as one POST; one that is down or
    /// refuses it is an error, never a panic or a wait past the timeout.
    #[test]
    fn batches_are_posted() {
        let collector = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = collector.local_addr().unwrap().to_string();
        let o = exporter(&addr);
        let got = std::thread::spawn(move || {
            let mut said = Vec::new();
            for answer in ["HTTP/1.1 200 OK\r\n\r\n", "HTTP/1.1 503 Busy\r\n\r\n"] {
                let (mut c, _) = collector.accept().unwrap();
                let mut buf = vec![0; 1 << 16];
                let mut all = Vec::new();
                while !String::from_utf8_lossy(&all).contains(r#"]}]}]}"#) {
                    let n = c.read(&mut buf).unwrap();
                    all.extend_from_slice(&buf[..n]);
                }
                c.write_all(answer.as_bytes()).unwrap();
                said.push(String::from_utf8(all).unwrap());
            }
            said
        });
        o.post(o.encode(&[server_span(200)]).as_bytes()).unwrap();
        let e = o.post(b"{}]}]}]}").unwrap_err();
        assert!(e.to_string().contains("503 Busy"), "{e}");
        let said = got.join().unwrap();
        assert!(
            said[0].starts_with("POST /v1/traces HTTP/1.1\r\n"),
            "{}",
            said[0]
        );
        assert!(
            said[0].contains("\r\nx-key: k\r\n")
                && said[0].contains("content-type: application/json")
        );
        let refused = TcpListener::bind("127.0.0.1:0").unwrap();
        let down = exporter(&refused.local_addr().unwrap().to_string());
        drop(refused);
        assert!(down.post(b"{}").is_err());
        down.ship(&[server_span(200)], &mut None); // says so, and goes on
    }

    #[test]
    fn exporter_headers() {
        assert_eq!(headers("api-key=a%20b, x=1,"), "api-key: a b\r\nx: 1\r\n");
    }
}
