//! What the server tells an operator about its requests, each part off
//! until its variable is set: `WISP_LOG=json`, a JSON line a request on
//! stdout. With none set, [`OBS`] stays empty and a request pays one load
//! of it.

use crate::Cx;
use crate::rt::RouteFacts;
use std::io::Write;
use std::net::IpAddr;
use std::sync::OnceLock;
use std::time::{Instant, SystemTime};

/// What is on, read from the environment once, at start. Empty when
/// nothing is.
static OBS: OnceLock<Obs> = OnceLock::new();

struct Obs {
    /// `WISP_LOG=json`.
    json: bool,
    /// The app's routes, for their patterns.
    routes: &'static [RouteFacts],
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
        if json {
            let _ = OBS.set(Obs { json, routes });
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
        Pending {
            started: Instant::now(),
            route,
            method: cx.method.as_str(),
            ip: cx.client_ip(),
            path: cx.path().to_owned(),
            id: if self.json {
                cx.request_id().to_owned()
            } else {
                String::new()
            },
        }
    }

    #[inline(never)]
    fn finish(&self, p: Pending, status: u16, bytes: usize) {
        if self.json {
            let route = p.route.and_then(|r| self.routes.get(r)).map(|r| r.pattern);
            let line = line(&p, route, status, bytes, unix_millis());
            // A reader that is gone loses the line, never the request.
            let _ = std::io::stdout().lock().write_all(line.as_bytes());
        }
    }
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
