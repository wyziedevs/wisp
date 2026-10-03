//! What the server tells an operator about its requests, each part off
//! until its variable is set: `WISP_LOG=json`, a JSON line a request on
//! stdout; `METRICS_KEY`, Prometheus text at `/_wisp/metrics`. With none
//! set, [`OBS`] stays empty and a request pays one load of it.

use crate::Cx;
use crate::http::{Body, Reply};
use crate::rt::RouteFacts;
use std::borrow::Cow;
use std::io::Write;
use std::net::IpAddr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant, SystemTime};

/// What is on, read from the environment once, at start. Empty when
/// nothing is.
static OBS: OnceLock<Obs> = OnceLock::new();

struct Obs {
    /// `WISP_LOG=json`.
    json: bool,
    /// `METRICS_KEY`.
    metrics: Option<Metrics>,
    /// The app's routes, for their patterns.
    routes: &'static [RouteFacts],
    started: Instant,
}

/// Reads the settings, once; a bad one stops the server at start. The
/// edge build has no clock to time requests by, and its host logs them.
pub(crate) fn init(routes: &'static [RouteFacts]) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if cfg!(target_arch = "wasm32") {
            return;
        }
        let json = match crate::setting::<String>("WISP_LOG", "json or off") {
            None => false,
            Some(v) if v.eq_ignore_ascii_case("json") => true,
            Some(v) if v.is_empty() || v.eq_ignore_ascii_case("off") => false,
            Some(v) => crate::fail(&format!("WISP_LOG is {v:?}, which is not json or off")),
        };
        let metrics = crate::setting::<String>("METRICS_KEY", "a key")
            .filter(|k| !k.is_empty())
            .map(|key| Metrics::new(key, routes.len()));
        if json || metrics.is_some() {
            let started = Instant::now();
            let _ = OBS.set(Obs {
                json,
                metrics,
                routes,
                started,
            });
        }
    });
}

/// A request under way, while something watches: what its line needs,
/// taken when it is decided, for when it is sent.
pub(crate) struct Pending {
    started: Instant,
    route: Option<usize>,
    method: &'static str,
    ip: IpAddr,
    path: String,
    id: String,
}

/// The start of the request in `cx`, routed to `route`, if anything
/// watches. `WISP_LOG=json` gives it an id, as `WISP_REQUEST_ID=on` does.
#[inline]
pub(crate) fn begin(cx: &Cx, route: Option<usize>) -> Option<Pending> {
    OBS.get().map(|o| o.begin(cx, route))
}

/// The request `p` answered, with `status` and a body of `bytes`.
#[inline]
pub(crate) fn finish(p: Pending, status: u16, bytes: usize) {
    if let Some(o) = OBS.get() {
        o.finish(p, status, bytes);
    }
}

impl Obs {
    #[inline(never)]
    fn begin(&self, cx: &Cx, route: Option<usize>) -> Pending {
        if let Some(m) = &self.metrics {
            m.in_flight.fetch_add(1, Relaxed);
        }
        // Metrics alone take nothing that allocates.
        let (path, id) = match self.json {
            true => (cx.path().to_owned(), cx.request_id().to_owned()),
            false => (String::new(), String::new()),
        };
        Pending {
            started: Instant::now(),
            route,
            method: cx.method.as_str(),
            ip: cx.client_ip(),
            path,
            id,
        }
    }

    #[inline(never)]
    fn finish(&self, p: Pending, status: u16, bytes: usize) {
        if let Some(m) = &self.metrics {
            m.record(p.route, status, p.started.elapsed());
        }
        if self.json {
            let route = p.route.and_then(|r| self.routes.get(r)).map(|r| r.pattern);
            let line = line(&p, route, status, bytes, unix_millis());
            // A reader that is gone loses the line, never the request.
            let _ = std::io::stdout().lock().write_all(line.as_bytes());
        }
    }
}

impl Drop for Pending {
    /// No longer in flight, answered or not (its connection gone).
    fn drop(&mut self) {
        if let Some(m) = OBS.get().and_then(|o| o.metrics.as_ref()) {
            m.in_flight.fetch_sub(1, Relaxed);
        }
    }
}

/// Upper bounds of the latency histogram's buckets, in microseconds:
/// Prometheus' usual ones, 1 ms to 10 s.
const BUCKETS: [u64; 13] = [
    1_000, 2_500, 5_000, 10_000, 25_000, 50_000, 100_000, 250_000, 500_000, 1_000_000, 2_500_000,
    5_000_000, 10_000_000,
];

/// `METRICS_KEY`'s counters: a row a route, made at start from the route
/// table, and one for requests no route matched. Atomics only, added to
/// with no lock.
struct Metrics {
    key: String,
    rows: Box<[Row]>,
    in_flight: AtomicI64,
}

/// One route's counts, on cache lines of its own: threads answering
/// other routes never write them.
#[derive(Default)]
#[repr(align(64))]
struct Row {
    /// By status class, 1xx to 5xx.
    classes: [AtomicU64; 5],
    /// By bucket, each request in one (made cumulative when served); one
    /// slower than the last is in none.
    buckets: [AtomicU64; BUCKETS.len()],
    micros: AtomicU64,
}

impl Metrics {
    fn new(key: String, routes: usize) -> Metrics {
        Metrics {
            key,
            rows: (0..=routes).map(|_| Row::default()).collect(),
            in_flight: AtomicI64::new(0),
        }
    }

    fn record(&self, route: Option<usize>, status: u16, took: Duration) {
        let last = self.rows.len() - 1;
        let row = &self.rows[route.map_or(last, |r| r.min(last))];
        let class = usize::from(status / 100).clamp(1, 5) - 1;
        row.classes[class].fetch_add(1, Relaxed);
        let micros = u64::try_from(took.as_micros()).unwrap_or(u64::MAX);
        if let Some(b) = BUCKETS.iter().position(|&le| micros <= le) {
            row.buckets[b].fetch_add(1, Relaxed);
        }
        row.micros.fetch_add(micros, Relaxed);
    }

    /// The Prometheus text: each route that had requests (`""` for none
    /// matched), then the process.
    fn render(&self, routes: &[RouteFacts], up: Duration, rss: Option<u64>) -> String {
        use std::fmt::Write;
        let mut s = String::with_capacity(4096);
        let mut rows = Vec::new();
        for (i, row) in self.rows.iter().enumerate() {
            let total: u64 = row.classes.iter().map(|c| c.load(Relaxed)).sum();
            if total > 0 {
                rows.push((label(routes.get(i).map_or("", |r| r.pattern)), row, total));
            }
        }
        s.push_str("# HELP wisp_requests_total Requests answered, by route and status class.\n");
        s.push_str("# TYPE wisp_requests_total counter\n");
        for (route, row, _) in &rows {
            for (k, c) in row.classes.iter().enumerate() {
                let n = c.load(Relaxed);
                if n > 0 {
                    let class = k + 1;
                    let _ = writeln!(
                        s,
                        "wisp_requests_total{{route=\"{route}\",status=\"{class}xx\"}} {n}"
                    );
                }
            }
        }
        s.push_str("# HELP wisp_request_duration_seconds Time to answer, by route.\n");
        s.push_str("# TYPE wisp_request_duration_seconds histogram\n");
        let series = "wisp_request_duration_seconds";
        for (route, row, total) in &rows {
            let mut below = 0;
            for (le, b) in BUCKETS.iter().zip(&row.buckets) {
                below += b.load(Relaxed);
                let le = *le as f64 / 1e6;
                let _ = writeln!(
                    s,
                    "{series}_bucket{{route=\"{route}\",le=\"{le}\"}} {below}"
                );
            }
            let sum = row.micros.load(Relaxed) as f64 / 1e6;
            let _ = writeln!(
                s,
                "{series}_bucket{{route=\"{route}\",le=\"+Inf\"}} {total}\n\
                 {series}_sum{{route=\"{route}\"}} {sum}\n\
                 {series}_count{{route=\"{route}\"}} {total}"
            );
        }
        let _ = writeln!(
            s,
            "# HELP wisp_requests_in_flight Requests being answered.\n\
             # TYPE wisp_requests_in_flight gauge\n\
             wisp_requests_in_flight {}\n\
             # HELP wisp_uptime_seconds Seconds since the server started.\n\
             # TYPE wisp_uptime_seconds gauge\n\
             wisp_uptime_seconds {:.3}",
            self.in_flight.load(Relaxed),
            up.as_secs_f64()
        );
        if let Some(rss) = rss {
            let _ = writeln!(
                s,
                "# HELP process_resident_memory_bytes Resident memory size in bytes.\n\
                 # TYPE process_resident_memory_bytes gauge\n\
                 process_resident_memory_bytes {rss}"
            );
        }
        s
    }
}

/// `v` as a label value, with `\`, `"` and newlines escaped.
fn label(v: &str) -> Cow<'_, str> {
    if !v.contains(['\\', '"', '\n']) {
        return Cow::Borrowed(v);
    }
    let v = v.replace('\\', "\\\\").replace('"', "\\\"");
    Cow::Owned(v.replace('\n', "\\n"))
}

/// The process's resident memory, where reading it is cheap (Linux's
/// `/proc`).
fn rss() -> Option<u64> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find_map(|l| l.strip_prefix("VmRSS:"))?;
    let kb: u64 = line.trim().strip_suffix("kB")?.trim().parse().ok()?;
    Some(kb * 1024)
}

/// `GET /_wisp/metrics` when `METRICS_KEY` is set (else `false`: the
/// path is the app's, a 404): the metrics for `Authorization: Bearer
/// <key>`, a 401 for anything else.
pub(crate) fn serve(cx: &Cx, reply: &mut Reply) -> bool {
    let Some((o, m)) = OBS.get().and_then(|o| Some((o, o.metrics.as_ref()?))) else {
        return false;
    };
    if !crate::secure_eq(cx.bearer().unwrap_or(""), &m.key) {
        reply.set_plain(401, "Unauthorized");
        let bearer = (Cow::Borrowed("www-authenticate"), Cow::Borrowed("Bearer"));
        reply.headers.push(bearer);
        return true;
    }
    let text = m.render(o.routes, o.started.elapsed(), rss());
    let kind = "text/plain; version=0.0.4; charset=utf-8";
    reply.set(200, kind, Body::Bytes(text.into_bytes()));
    let fresh = (Cow::Borrowed("cache-control"), Cow::Borrowed("no-store"));
    reply.headers.push(fresh);
    true
}

/// Milliseconds since 1970, by the wall clock.
fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The JSON line of `p`, ended by a newline.
fn line(p: &Pending, route: Option<&str>, status: u16, bytes: usize, millis: u64) -> String {
    use crate::Json;
    use std::fmt::Write;
    let mut s = String::with_capacity(160 + p.path.len() + p.id.len());
    s.push_str("{\"time\":\"");
    rfc3339(&mut s, millis);
    let _ = write!(s, "\",\"method\":\"{}\",\"route\":", p.method);
    match route {
        Some(r) => r.json(&mut s),
        None => s.push_str("null"),
    }
    s.push_str(",\"path\":");
    p.path.json(&mut s);
    let ms = p.started.elapsed().as_secs_f64() * 1000.0;
    let _ = write!(
        s,
        ",\"status\":{status},\"ms\":{ms:.3},\"bytes\":{bytes},\"id\":"
    );
    p.id.json(&mut s);
    let _ = writeln!(s, ",\"ip\":\"{}\"}}", p.ip);
    s
}

/// `millis` since 1970 as `2026-10-03T12:00:00.123Z`.
fn rfc3339(s: &mut String, millis: u64) {
    use std::fmt::Write;
    let (secs, ms) = (millis / 1000, millis % 1000);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (y, mo, d) = crate::civil(days);
    let (h, mi, se) = (rem / 3600, rem % 3600 / 60, rem % 60);
    let _ = write!(s, "{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{se:02}.{ms:03}Z");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(path: &str) -> Pending {
        Pending {
            started: Instant::now(),
            route: Some(0),
            method: "GET",
            ip: IpAddr::from([10, 0, 0, 7]),
            path: path.into(),
            id: "ab\"c".into(),
        }
    }

    #[test]
    fn metrics_count_by_route_class_and_bucket() {
        const ROUTES: [RouteFacts; 2] = [
            RouteFacts {
                pattern: "/",
                ..RouteFacts::new(&[])
            },
            RouteFacts {
                pattern: "/a\"b",
                ..RouteFacts::new(&[])
            },
        ];
        let m = Metrics::new("k".into(), ROUTES.len());
        let ms = Duration::from_millis;
        m.record(Some(0), 200, ms(0));
        m.record(Some(0), 304, ms(3));
        m.record(Some(0), 503, ms(20_000));
        m.record(None, 404, ms(1));
        m.record(Some(99), 999, ms(1)); // another app's id: counted, not out of bounds
        let text = m.render(&ROUTES, Duration::from_secs(2), Some(4096));
        for want in [
            "wisp_requests_total{route=\"/\",status=\"2xx\"} 1\n",
            "wisp_requests_total{route=\"/\",status=\"3xx\"} 1\n",
            "wisp_requests_total{route=\"/\",status=\"5xx\"} 1\n",
            "wisp_requests_total{route=\"\",status=\"4xx\"} 1\n",
            "wisp_requests_total{route=\"\",status=\"5xx\"} 1\n",
            "wisp_request_duration_seconds_bucket{route=\"/\",le=\"0.001\"} 1\n",
            "wisp_request_duration_seconds_bucket{route=\"/\",le=\"0.0025\"} 1\n",
            "wisp_request_duration_seconds_bucket{route=\"/\",le=\"0.005\"} 2\n",
            "wisp_request_duration_seconds_bucket{route=\"/\",le=\"10\"} 2\n",
            "wisp_request_duration_seconds_bucket{route=\"/\",le=\"+Inf\"} 3\n",
            "wisp_request_duration_seconds_sum{route=\"/\"} 20.003\n",
            "wisp_request_duration_seconds_count{route=\"/\"} 3\n",
            "wisp_requests_in_flight 0\n",
            "wisp_uptime_seconds 2.000\n",
            "process_resident_memory_bytes 4096\n",
        ] {
            assert!(text.contains(want), "{want}\n{text}");
        }
        assert!(
            !text.contains("a\"b"),
            "a route with no requests is left out"
        );
        assert_eq!(label("/a\"b\\\n"), "/a\\\"b\\\\\\n");
    }

    #[test]
    fn dates() {
        let mut s = String::new();
        rfc3339(&mut s, 0);
        assert_eq!(s, "1970-01-01T00:00:00.000Z");
        s.clear();
        rfc3339(&mut s, 1_791_029_045_007);
        assert_eq!(s, "2026-10-03T12:04:05.007Z");
    }

    #[test]
    fn a_line_is_one_json_object() {
        let text = line(&pending("/blog/\"x\"\n"), Some("/blog/[slug]"), 404, 12, 0);
        assert!(
            text.ends_with("}\n") && text.matches('\n').count() == 1,
            "{text}"
        );
        let v = crate::json::parse(&text).unwrap();
        assert_eq!(
            v.get("time").and_then(|v| v.as_str()),
            Some("1970-01-01T00:00:00.000Z")
        );
        assert_eq!(v.get("method").and_then(|v| v.as_str()), Some("GET"));
        assert_eq!(
            v.get("route").and_then(|v| v.as_str()),
            Some("/blog/[slug]")
        );
        assert_eq!(
            v.get("path").and_then(|v| v.as_str()),
            Some("/blog/\"x\"\n")
        );
        assert_eq!(v.get("status").and_then(|v| v.as_i64()), Some(404));
        assert_eq!(v.get("bytes").and_then(|v| v.as_i64()), Some(12));
        assert_eq!(v.get("id").and_then(|v| v.as_str()), Some("ab\"c"));
        assert_eq!(v.get("ip").and_then(|v| v.as_str()), Some("10.0.0.7"));
        assert!(
            v.get("ms")
                .and_then(|v| v.as_f64())
                .is_some_and(|ms| ms >= 0.0)
        );
        let unrouted = line(&pending("/x"), None, 404, 0, 0);
        assert!(
            crate::json::parse(&unrouted)
                .unwrap()
                .get("route")
                .unwrap()
                .is_null()
        );
    }
}
