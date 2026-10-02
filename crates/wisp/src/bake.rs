//! Responses sent as bytes made before. A page the compiler proved
//! constant (it, its layouts and components read nothing of the request)
//! is baked into the binary whole: status line, `content-type`,
//! `content-length`, ETag and document. A page or GET endpoint with
//! `const CACHE: u32 = N;` is kept by each worker, after a render, for N
//! seconds. Either way, answering is two copies and the date, and a
//! conditional GET gets its 304 without hashing a byte.
//!
//! What `CACHE` keeps is shared only by requests that carry no cookie and
//! no `authorization` (`CACHE_PUBLIC` shares it with every request), and
//! never holds a response that sets a cookie or says `private` or
//! `no-store`. Hooks still run on every request. Dev mode keeps nothing.

use crate::http::{Body, Reply};
use crate::{App, Cx, Out};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

/// A page the compiler found constant, as it goes on the wire: the status
/// line and fixed headers (`content-length` among them), the document, and
/// its ETag (quoted).
pub struct Baked {
    head: &'static str,
    body: &'static str,
    etag: &'static str,
}

impl Baked {
    pub const fn new(head: &'static str, body: &'static str, etag: &'static str) -> Baked {
        Baked { head, body, etag }
    }
}

/// A response `CACHE` keeps: head then body in `wire`, split at `head`,
/// and the unix second it is fresh until.
pub struct Kept {
    wire: Box<[u8]>,
    head: usize,
    etag: Box<str>,
    until: u64,
}

/// A made response, as a reply holds it.
#[derive(Clone)]
pub enum Made {
    Baked(&'static Baked),
    Kept(Arc<Kept>),
}

impl Made {
    /// The status line and fixed headers, each line ended.
    pub(crate) fn head(&self) -> &[u8] {
        match self {
            Made::Baked(b) => b.head.as_bytes(),
            Made::Kept(k) => &k.wire[..k.head],
        }
    }

    pub(crate) fn body(&self) -> &[u8] {
        match self {
            Made::Baked(b) => b.body.as_bytes(),
            Made::Kept(k) => &k.wire[k.head..],
        }
    }

    fn etag(&self) -> &str {
        match self {
            Made::Baked(b) => b.etag,
            Made::Kept(k) => &k.etag,
        }
    }
}

/// A baked page's GET: answered with the page, unless in dev mode (whose
/// templates change without a build) or `before` set a status. `false`:
/// render it.
pub fn baked(cx: &Cx, out: &mut Out, page: &'static Baked) -> bool {
    if crate::settings().dev || cx.status() != 200 {
        return false;
    }
    out.made = Some(Made::Baked(page));
    true
}

/// The reply to a made response: a 304 when the request's `if-none-match`
/// names its ETag, with the headers a 200 would have but those of its body,
/// else its bytes.
pub(crate) fn reply(cx: &Cx, made: Made, reply: &mut Reply) {
    reply.headers.clear();
    if !cx
        .header("if-none-match")
        .is_some_and(|h| crate::rest::names::<true>(h, made.etag()))
    {
        reply.status = 200;
        reply.body = Body::Made(made);
        return;
    }
    reply.status = 304;
    reply.body = Body::Static(b"");
    match made {
        // Its head is the type and length of its body, and the ETag.
        Made::Baked(b) => reply
            .headers
            .push((Cow::Borrowed("etag"), Cow::Borrowed(b.etag))),
        Made::Kept(k) => reply.headers.extend(
            headers(&k.wire[..k.head]).filter(|(n, _)| !n.eq_ignore_ascii_case("content-type")),
        ),
    }
}

/// A made reply as hosts other than the built-in server take it: the
/// head's headers as pairs, first, and the body.
pub(crate) fn unpack(reply: &mut Reply) {
    let Body::Made(made) = std::mem::replace(&mut reply.body, Body::Static(b"")) else {
        return;
    };
    reply.headers.splice(0..0, headers(made.head()));
    reply.body = match made {
        Made::Baked(b) => Body::Static(b.body.as_bytes()),
        Made::Kept(k) => Body::Bytes(k.wire[k.head..].to_vec()),
    };
}

/// The headers of a made head as pairs, but `content-length`: framing,
/// which is for whoever sends it.
fn headers(head: &[u8]) -> impl Iterator<Item = (Cow<'static, str>, Cow<'static, str>)> {
    let head = String::from_utf8_lossy(head);
    let pairs: Vec<_> = (head.split("\r\n").skip(1))
        .filter_map(|line| line.split_once(": "))
        .filter(|(name, _)| !name.eq_ignore_ascii_case("content-length"))
        .map(|(n, v)| (Cow::Owned(n.to_string()), Cow::Owned(v.to_string())))
        .collect();
    pairs.into_iter()
}

/// Bytes of responses one worker keeps at most (their keys too). Past it,
/// the stale ones go, and one that still does not fit is not kept: a flood
/// of new addresses costs renders, never memory.
const BUDGET: usize = 8 * 1024 * 1024;

/// One worker's kept responses by key, the bytes they take of `budget`,
/// and the buffer a request's key is written into.
struct Store {
    kept: HashMap<Box<[u8]>, Arc<Kept>>,
    bytes: usize,
    budget: usize,
    key: Vec<u8>,
}

impl Store {
    fn new(budget: usize) -> Store {
        Store {
            kept: HashMap::new(),
            bytes: 0,
            budget,
            key: Vec::new(),
        }
    }

    /// The response kept for `cx`, while it is fresh.
    fn get<const ACCEPT: bool>(&mut self, cx: &Cx, now: u64) -> Option<Arc<Kept>> {
        let k = self.kept.get(key::<ACCEPT>(&mut self.key, cx))?;
        (k.until > now).then(|| k.clone())
    }

    /// Keeps `k` under `key`, in place of what was there, if it fits the
    /// budget once the stale ones are gone.
    fn insert(&mut self, key: Box<[u8]>, k: Arc<Kept>, now: u64) {
        let need = size(&key, &k);
        if self.bytes + need > self.budget {
            self.kept.retain(|_, k| k.until > now);
            self.bytes = self.kept.iter().map(|(key, k)| size(key, k)).sum();
        }
        let old = self.kept.get(&key).map_or(0, |old| size(&key, old));
        if self.bytes - old + need <= self.budget {
            self.bytes = self.bytes - old + need;
            self.kept.insert(key, k);
        }
    }
}

/// The key of `cx` in `buf`: `Host`, a NUL, then the path and query; and
/// with `ACCEPT` (a route whose answer varies by `accept`), a NUL and what
/// it asks for: `n` for NDJSON, `j` for JSON.
fn key<'k, const ACCEPT: bool>(buf: &'k mut Vec<u8>, cx: &Cx) -> &'k [u8] {
    buf.clear();
    buf.extend_from_slice(cx.header("host").unwrap_or("").as_bytes());
    buf.push(0);
    buf.extend_from_slice(cx.path().as_bytes());
    if !cx.query_string().is_empty() {
        buf.push(b'?');
        buf.extend_from_slice(cx.query_string().as_bytes());
    }
    if ACCEPT {
        let asked: &[u8] = if crate::rest::lines(cx) {
            b"\0n"
        } else {
            b"\0j"
        };
        buf.extend_from_slice(asked);
    }
    buf
}

/// What an entry takes of the budget.
fn size(key: &[u8], k: &Kept) -> usize {
    key.len() + k.wire.len() + k.etag.len()
}

thread_local! {
    /// Per worker, so it takes no lock: the thread is the server's (or the
    /// host's) and keeps it for as long as it runs.
    static STORE: RefCell<Store> = RefCell::new(Store::new(BUDGET));
}

/// Whether the request may be answered from what workers keep, and its
/// answer kept: not in dev mode, and, unless the route is public, only a
/// request without a cookie or `authorization`, which could make the
/// answer its own.
fn shared(cx: &Cx, public: bool) -> bool {
    !crate::settings().dev
        && (public || (cx.header("cookie").is_none() && cx.header("authorization").is_none()))
}

/// A header that makes a response its visitor's alone: a cookie set, or a
/// `cache-control` of `private` or `no-store`.
fn personal(name: &str, value: &str) -> bool {
    name.eq_ignore_ascii_case("set-cookie")
        || (name.eq_ignore_ascii_case("cache-control")
            && ["private", "no-store"].iter().any(|d| value.contains(d)))
}

/// A `CACHE` route's GET: answered with the response this worker keeps for
/// it, when it has a fresh one. `false`: render it (then [`keep`]).
/// `ACCEPT` for a route whose answer varies by `accept` (a
/// `#[derive(Rest)]` list: JSON or NDJSON), which keeps each apart; the
/// build sets it for those routes alone.
#[inline(always)]
pub fn cached<const ACCEPT: bool>(cx: &Cx, out: &mut Out, public: bool) -> bool {
    if !shared(cx, public) {
        return false;
    }
    let now = crate::http::now();
    out.made = STORE
        .with_borrow_mut(|s| s.get::<ACCEPT>(cx, now))
        .map(Made::Kept);
    out.made.is_some()
}

/// Keeps what a `CACHE` route just answered for `secs` seconds, and
/// answers with it: a 200 page, or an endpoint's whole response, without a
/// header that makes it personal. Headers the route set are kept with it;
/// those of the `before` hook, which runs every time, are not. `ACCEPT`
/// as for [`cached`].
#[inline(always)]
pub fn keep<A: App, const ACCEPT: bool>(cx: &mut Cx, out: &mut Out, secs: u32, public: bool) {
    if secs == 0 || cx.status() != 200 || out.made.is_some() || !shared(cx, public) {
        return;
    }
    if cx.page_headers().iter().any(|(n, v)| personal(n, v)) {
        return;
    }
    let mut head = Vec::with_capacity(256);
    head.extend_from_slice(b"HTTP/1.1 200 OK\r\n");
    let body = match out.response.take() {
        None => {
            line(&mut head, "content-type", "text/html; charset=utf-8");
            crate::http::page::<A>(out).concat().into_bytes()
        }
        Some(r)
            if r.status == 200
                && r.stream.is_none()
                && r.upgrade.is_none()
                && !r.headers.iter().any(|(n, v)| personal(n, v)) =>
        {
            if !r.content_type.is_empty() {
                line(&mut head, "content-type", &r.content_type);
            }
            for (n, v) in &r.headers {
                line(&mut head, n, v);
            }
            r.body
        }
        Some(r) => {
            out.response = Some(r);
            return;
        }
    };
    for (n, v) in cx.take_page_headers() {
        line(&mut head, &n, &v);
    }
    // An endpoint's own ETag (a REST row's) stands; any other gets one.
    let etag = match header_in(&head, "etag") {
        Some(tag) => tag.to_string(),
        None => {
            let tag = crate::rest::etag(&body);
            line(&mut head, "etag", &tag);
            tag
        }
    };
    head.extend_from_slice(b"content-length: ");
    head.extend_from_slice(body.len().to_string().as_bytes());
    head.extend_from_slice(b"\r\n");
    let split = head.len();
    head.extend_from_slice(&body);
    let now = crate::http::now();
    let kept = Arc::new(Kept {
        wire: head.into_boxed_slice(),
        head: split,
        etag: etag.into_boxed_str(),
        until: now + u64::from(secs),
    });
    STORE.with_borrow_mut(|s| {
        let key = key::<ACCEPT>(&mut s.key, cx).into();
        s.insert(key, kept.clone(), now);
    });
    out.made = Some(Made::Kept(kept));
}

/// Adds `name: value` to a head, unless it would split the response or is
/// framing, which the head has its own of.
fn line(head: &mut Vec<u8>, name: &str, value: &str) {
    let framing = name.eq_ignore_ascii_case("content-length")
        || name.eq_ignore_ascii_case("transfer-encoding");
    if framing || !crate::cx::valid_header(name, value) {
        return;
    }
    for part in [name.as_bytes(), b": ", value.as_bytes(), b"\r\n"] {
        head.extend_from_slice(part);
    }
}

/// The value of header `name` in a head written by [`line`].
fn header_in<'h>(head: &'h [u8], name: &str) -> Option<&'h str> {
    std::str::from_utf8(head)
        .ok()?
        .split("\r\n")
        .filter_map(|l| l.split_once(": "))
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fuzz::Fuzz;

    static PAGE: Baked = Baked::new(
        "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\netag: \"t\"\r\ncontent-length: 2\r\n",
        "hi",
        "\"t\"",
    );

    fn cx(headers: &[(&str, &str)]) -> Cx {
        let headers = headers.iter().map(|(n, v)| (*n, v.as_bytes()));
        Cx::from_request::<Fuzz>("GET", "/p?q=1", headers, b"", ([127, 0, 0, 1], 1).into()).unwrap()
    }

    fn kept(bytes: usize, until: u64) -> Arc<Kept> {
        Arc::new(Kept {
            wire: vec![b'x'; bytes].into(),
            head: 0,
            etag: "".into(),
            until,
        })
    }

    #[test]
    fn made_replies_and_their_304() {
        let mut r = Reply::default();
        reply(&cx(&[]), Made::Baked(&PAGE), &mut r);
        assert_eq!((r.status, r.bytes()), (200, &b"hi"[..]));

        // Other hosts get it as headers, the hook's after its own.
        r.headers
            .push((Cow::Borrowed("x-hook"), Cow::Borrowed("1")));
        unpack(&mut r);
        let headers: Vec<(&str, &str)> = r.headers.iter().map(|(n, v)| (&**n, &**v)).collect();
        assert_eq!(
            headers,
            [
                ("content-type", "text/html; charset=utf-8"),
                ("etag", "\"t\""),
                ("x-hook", "1")
            ]
        );
        assert!(matches!(r.body, Body::Static(b"hi")));

        for asked in ["\"t\"", "W/\"t\"", "\"a\", \"t\"", "*"] {
            reply(&cx(&[("if-none-match", asked)]), Made::Baked(&PAGE), &mut r);
            assert_eq!(
                (r.status, r.bytes(), r.header("etag")),
                (304, &b""[..], Some("\"t\""))
            );
        }
        reply(
            &cx(&[("if-none-match", "\"u\"")]),
            Made::Baked(&PAGE),
            &mut r,
        );
        assert_eq!(r.status, 200);

        // A kept one's 304 has the headers its route set too.
        let head = "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncache-control: max-age=5\r\netag: \"k\"\r\ncontent-length: 2\r\n";
        let k = Arc::new(Kept {
            wire: format!("{head}ok").into_bytes().into(),
            head: head.len(),
            etag: "\"k\"".into(),
            until: u64::MAX,
        });
        reply(&cx(&[("if-none-match", "\"k\"")]), Made::Kept(k), &mut r);
        let headers: Vec<(&str, &str)> = r.headers.iter().map(|(n, v)| (&**n, &**v)).collect();
        assert_eq!(
            (r.status, headers),
            (304, vec![("cache-control", "max-age=5"), ("etag", "\"k\"")])
        );
    }

    #[test]
    fn keys_and_what_is_never_kept() {
        let mut buf = Vec::new();
        assert_eq!(
            key::<false>(&mut buf, &cx(&[("host", "a.test")])),
            b"a.test\0/p?q=1"
        );
        // A list's JSON and NDJSON are kept apart.
        let json = key::<true>(&mut buf, &cx(&[("accept", "application/json")])).to_vec();
        let lines = key::<true>(&mut buf, &cx(&[("accept", "application/x-ndjson")])).to_vec();
        assert_eq!(
            (&json[..], &lines[..]),
            (&b"\0/p?q=1\0j"[..], &b"\0/p?q=1\0n"[..])
        );
        assert!(personal("Set-Cookie", "a=1"));
        assert!(personal("cache-control", "private, max-age=0"));
        assert!(personal("Cache-Control", "no-store"));
        assert!(!personal("cache-control", "public, max-age=60"));
        assert!(!personal("x-note", "private"));
        let mut head = Vec::new();
        line(&mut head, "content-length", "9");
        line(&mut head, "x-bad", "a\r\nset-cookie: b");
        line(&mut head, "etag", "\"e\"");
        assert_eq!(head, b"etag: \"e\"\r\n");
        assert_eq!(header_in(&head, "ETag"), Some("\"e\""));
    }

    #[test]
    fn a_worker_keeps_within_its_budget() {
        let mut s = Store::new(100);
        let one = cx(&[]);
        s.insert(Box::from(&b"a"[..]), kept(40, 10), 0);
        s.insert(Box::from(&b"b"[..]), kept(40, 20), 0);
        // Too big with those two: not kept.
        s.insert(Box::from(&b"c"[..]), kept(40, 20), 5);
        assert_eq!((s.kept.len(), s.bytes), (2, 82));
        // Once `a` is stale, it goes to make room.
        s.insert(Box::from(&b"c"[..]), kept(40, 20), 10);
        assert!(s.kept.contains_key(&b"c"[..]) && !s.kept.contains_key(&b"a"[..]));
        // A key kept again takes its old room.
        s.insert(Box::from(&b"c"[..]), kept(50, 30), 10);
        assert_eq!(s.bytes, 41 + 51);

        let mut buf = Vec::new();
        let at = key::<false>(&mut buf, &one).to_vec();
        s.insert(at.into(), kept(1, 10), 0);
        assert!(s.get::<false>(&one, 9).is_some() && s.get::<false>(&one, 10).is_none());
        assert!(s.get::<true>(&one, 9).is_none());
    }
}
