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

use crate::http::{Body, Reply, Request};
use crate::{App, Cx, Out};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

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

/// What a route's `CACHE` has besides its seconds: `const CACHE_STALE: u32`,
/// the seconds after it ends that the old answer is still sent while one
/// request makes a new one, and `const CACHE_TAGS: &[&str]`, names for
/// `wisp::revalidate_tag`. Consts, so a route with neither costs nothing.
#[derive(Clone, Copy)]
pub struct CacheMore {
    stale: u32,
    tags: &'static [&'static str],
}

impl CacheMore {
    pub const NONE: CacheMore = CacheMore::new(0, &[]);

    pub const fn new(stale: u32, tags: &'static [&'static str]) -> CacheMore {
        CacheMore { stale, tags }
    }
}

/// How a stale answer is made again: the app's own `handle`.
type Refresh = fn(Request) -> Pin<Box<dyn Future<Output = Reply> + Send>>;

fn refresh_with<A: App>(req: Request) -> Pin<Box<dyn Future<Output = Reply> + Send>> {
    Box::pin(crate::http::handle::<A>(req))
}

/// A response `CACHE` keeps: head then body in `wire`, split at `head`,
/// the unix second it is fresh until, and the one it may still be sent
/// until (`CACHE_STALE`), while `refreshing` says a request is making it
/// again.
pub struct Kept {
    wire: Box<[u8]>,
    head: usize,
    etag: Box<str>,
    until: u64,
    dies: u64,
    refreshing: AtomicBool,
    refresh: Option<Refresh>,
}

/// A made response, as a reply holds it.
#[derive(Clone)]
pub enum Made {
    Baked(&'static Baked),
    Kept(Arc<Kept>),
    /// A 204, of a handler that returns nothing.
    NoContent,
}

impl Made {
    /// The status line and fixed headers, each line ended.
    pub(crate) fn head(&self) -> &[u8] {
        match self {
            Made::Baked(b) => b.head.as_bytes(),
            Made::Kept(k) => &k.wire[..k.head],
            Made::NoContent => b"HTTP/1.1 204 No Content\r\n",
        }
    }

    pub(crate) fn body(&self) -> &[u8] {
        match self {
            Made::Baked(b) => b.body.as_bytes(),
            Made::Kept(k) => &k.wire[k.head..],
            Made::NoContent => b"",
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
    let fresh = match &made {
        Made::Baked(b) => crate::http::fresh(cx, b.etag),
        Made::Kept(k) => crate::http::fresh(cx, &k.etag),
        // As bytes, which an `Idempotency-Key` keeps.
        Made::NoContent => {
            (reply.status, reply.body) = (204, Body::Static(b""));
            return;
        }
    };
    if !fresh {
        // A kept page has its policy in its head already.
        if let (Made::Baked(_), Some(policy)) = (&made, crate::csp::header()) {
            reply.headers.push((
                Cow::Borrowed("content-security-policy"),
                Cow::Borrowed(policy),
            ));
        }
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
        Made::NoContent => {}
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
        Made::NoContent => Body::Static(b""),
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
    /// How many `uncache` calls this worker has seen.
    purged: u64,
    /// The keys kept under each tag: written when a tagged response is
    /// kept and when a tag is dropped, never when one is looked up. It may
    /// name a key that has gone; dropping that is a no-op.
    tags: HashMap<Box<str>, HashSet<Box<[u8]>>>,
}

/// How many times `uncache` was called, and the last of its prefixes with
/// the number of the call that gave each: a worker reads what it missed
/// the next time it looks for a kept page.
static PURGED: AtomicU64 = AtomicU64::new(0);
static PURGES: Mutex<Vec<(u64, String)>> = Mutex::new(Vec::new());

/// The most prefixes remembered: a worker further behind drops everything.
const PURGES_KEPT: usize = 64;

/// Has every worker drop what is kept under `tag` (a tag goes in the list
/// of prefixes behind a NUL, which no path starts with).
pub(crate) fn purge_tag(tag: &str) {
    purge(&format!("\0{tag}"));
}

/// Has every worker drop the kept pages under `prefix`, a path: that page
/// and those below it (`/posts` and `/posts/1`, not `/postscript`);
/// `/` drops all. Each does so before it next answers from what it keeps.
pub(crate) fn purge(prefix: &str) {
    let mut all = PURGES.lock().unwrap_or_else(|e| e.into_inner());
    let n = PURGED.load(Ordering::Relaxed) + 1;
    all.push((n, prefix.to_owned()));
    if all.len() > PURGES_KEPT {
        all.remove(0);
    }
    PURGED.store(n, Ordering::Release);
}

/// Whether the request path in `key` (see [`key`]) is `prefix` or below it.
fn under(key: &[u8], prefix: &str) -> bool {
    let after_host = key.iter().position(|&b| b == 0).map_or(0, |i| i + 1);
    let path = &key[after_host..];
    let end = path
        .iter()
        .position(|&b| b == b'?' || b == 0)
        .unwrap_or(path.len());
    let path = &path[..end];
    let p = prefix.as_bytes();
    path.starts_with(p) && (prefix.ends_with('/') || path.len() == p.len() || path[p.len()] == b'/')
}

impl Store {
    fn new(budget: usize) -> Store {
        Store {
            kept: HashMap::new(),
            bytes: 0,
            budget,
            key: Vec::new(),
            purged: PURGED.load(Ordering::Acquire),
            tags: HashMap::new(),
        }
    }

    /// Drops what `uncache` has been told to since this worker last looked.
    fn sync(&mut self) {
        let now = PURGED.load(Ordering::Relaxed);
        if now == self.purged {
            return;
        }
        let all = PURGES.lock().unwrap_or_else(|e| e.into_inner());
        if all.first().is_none_or(|(n, _)| *n > self.purged + 1) {
            self.kept.clear();
        } else {
            for (_, prefix) in all.iter().filter(|(n, _)| *n > self.purged) {
                match prefix.strip_prefix('\0') {
                    Some(tag) => {
                        for key in self.tags.remove(tag).iter().flatten() {
                            self.kept.remove(key);
                        }
                    }
                    None => self.kept.retain(|key, _| !under(key, prefix)),
                }
            }
        }
        self.purged = now;
        self.prune();
    }

    /// Forgets the tag entries of keys that are gone, and counts the bytes.
    fn prune(&mut self) {
        if !self.tags.is_empty() {
            let kept = &self.kept;
            self.tags.retain(|_, keys| {
                keys.retain(|k| kept.contains_key(k));
                !keys.is_empty()
            });
        }
        self.bytes = self.kept.iter().map(|(key, k)| size(key, k)).sum();
    }

    /// The response kept for `cx`, while it is fresh.
    fn get<const ACCEPT: bool>(&mut self, cx: &Cx, now: u64) -> Option<Arc<Kept>> {
        self.sync();
        let k = self.kept.get(key::<ACCEPT>(&mut self.key, cx))?;
        if k.until > now {
            return Some(k.clone());
        }
        match k.dies > now {
            true => stale(cx, k),
            false => None,
        }
    }

    /// Keeps `k` under `key`, in place of what was there, if it fits the
    /// budget once the dead ones are gone; whether it did.
    fn insert(&mut self, key: Box<[u8]>, k: Arc<Kept>, now: u64) -> bool {
        let need = size(&key, &k);
        if self.bytes + need > self.budget {
            self.kept.retain(|_, k| k.dies > now);
            self.prune();
        }
        let old = self.kept.get(&key).map_or(0, |old| size(&key, old));
        let fits = self.bytes - old + need <= self.budget;
        if fits {
            self.bytes = self.bytes - old + need;
            self.kept.insert(key, k);
        }
        fits
    }

    /// Files `key` under each of `tags`.
    #[cold]
    fn tag(&mut self, key: &[u8], tags: Vec<String>) {
        for t in tags {
            self.tags.entry(t.into()).or_default().insert(key.into());
        }
    }
}

/// A kept answer past its time but within its `CACHE_STALE`: sent as it is,
/// and made again, by one request at a time, in the background. The
/// request that does so (its peer is port 0, which a client never has)
/// is answered with a new one instead. Cold: only past the fresh time.
#[cold]
#[inline(never)]
fn stale(cx: &Cx, k: &Arc<Kept>) -> Option<Arc<Kept>> {
    if cx.peer().port() == 0 {
        return None;
    }
    if let Some(refresh) = k.refresh
        && !k.refreshing.swap(true, Ordering::Relaxed)
    {
        let mut req = Request::new("GET", &target(cx));
        req.headers = cx.headers().map(|(n, v)| (n.into(), v.into())).collect();
        req.peer = ([0, 0, 0, 0], 0).into();
        let k = k.clone();
        crate::spawn(async move {
            refresh(req).await;
            // A refresh that kept nothing (an error) may be tried again.
            k.refreshing.store(false, Ordering::Relaxed);
        });
    }
    Some(k.clone())
}

/// The request's path and query.
fn target(cx: &Cx) -> String {
    match cx.query_string() {
        "" => cx.path().to_owned(),
        q => format!("{}?{q}", cx.path()),
    }
}

/// The tags a response is kept under, besides its route's: see
/// `Cx::cache_tag`.
#[derive(Default)]
pub(crate) struct Tags(pub Vec<String>);

/// Whether this request is a draft one (see `Cx::draft`) to a route that
/// shares its answer with those who have cookies: it gets a render of its
/// own. A cookie-less request is not, and a route that does not share by
/// cookie never gets here with one: its cookie keeps it apart already.
#[inline(always)]
fn drafting(cx: &Cx, public: bool) -> bool {
    public && cx.header("cookie").is_some() && drafted(cx)
}

#[cold]
#[inline(never)]
fn drafted(cx: &Cx) -> bool {
    cx.draft()
}

/// The key of `cx` in `buf`: `Host`, a NUL, then the path and query; and
/// with `ACCEPT` (a route whose answer varies by `accept`), a NUL and what
/// it asks for: `n` for NDJSON, `j` for JSON; in an app with locales, a
/// NUL, `l` and the request's locale.
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
    // A page in another locale is another answer.
    if !crate::locales().is_empty() {
        buf.extend_from_slice(&[0, b'l', crate::i18n::pick(cx)]);
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
    if out.made.is_some() && drafting(cx, public) {
        out.made = None;
    }
    out.made.is_some()
}

/// Keeps what a `CACHE` route just answered for `secs` seconds, and
/// answers with it: a 200 page, or an endpoint's whole response, without a
/// header that makes it personal. Headers the route set are kept with it;
/// those of the `before` hook, which runs every time, are not. `ACCEPT`
/// as for [`cached`].
#[inline(always)]
pub fn keep<A: App, const ACCEPT: bool>(
    cx: &mut Cx,
    out: &mut Out,
    secs: u32,
    public: bool,
    more: CacheMore,
) {
    if secs == 0
        || cx.status() != 200
        || out.made.is_some()
        || !shared(cx, public)
        || drafting(cx, public)
    {
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
            if let Some(policy) = crate::csp::header() {
                line(&mut head, "content-security-policy", policy);
            }
            crate::headers::page(cx);
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
            if r.page {
                for (n, v) in crate::headers::missing(cx, &r.headers) {
                    line(&mut head, n, v);
                }
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
    let until = now + u64::from(secs);
    let kept = Arc::new(Kept {
        wire: head.into_boxed_slice(),
        head: split,
        etag: etag.into_boxed_str(),
        until,
        dies: until + u64::from(more.stale),
        refreshing: AtomicBool::new(false),
        refresh: (more.stale > 0).then_some(refresh_with::<A> as Refresh),
    });
    let tags = tags_of(cx, more);
    STORE.with_borrow_mut(|s| {
        let key: Box<[u8]> = key::<ACCEPT>(&mut s.key, cx).into();
        let tagged = (!tags.is_empty()).then(|| key.clone());
        if s.insert(key, kept.clone(), now)
            && let Some(key) = tagged
        {
            s.tag(&key, tags);
        }
    });
    out.made = Some(Made::Kept(kept));
}

/// The route's tags and those the handler gave: none for a route with
/// neither.
#[inline(always)]
fn tags_of(cx: &mut Cx, more: CacheMore) -> Vec<String> {
    let mut all: Vec<String> = more.tags.iter().map(|t| (*t).to_owned()).collect();
    if let Some(t) = cx.take::<Tags>() {
        all.extend(t.0);
    }
    all
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
            dies: until,
            refreshing: AtomicBool::new(false),
            refresh: None,
        })
    }

    #[test]
    fn uncache_drops_a_path_and_below() {
        let mut s = Store::new(1 << 20);
        let put = |s: &mut Store, key: &[u8]| {
            s.kept.insert(key.into(), kept(1, u64::MAX));
        };
        for key in [
            &b"h\0/uncache-test"[..],
            b"h\0/uncache-test?page=2",
            b"h\0/uncache-test/1\0j",
            b"h\0/uncache-tests",
            b"h\0/other",
        ] {
            put(&mut s, key);
        }
        purge("/uncache-test");
        s.sync();
        let mut left: Vec<_> = s
            .kept
            .keys()
            .map(|k| String::from_utf8_lossy(k).into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["h\0/other", "h\0/uncache-tests"]);
        assert_eq!(
            s.bytes,
            s.kept.iter().map(|(k, v)| size(k, v)).sum::<usize>()
        );
        // A prefix ending in "/" is everything below it, whatever follows.
        purge("/uncache-t/");
        put(&mut s, b"h\0/uncache-t/x");
        s.sync();
        assert_eq!(s.kept.len(), 2);
        put(&mut s, b"h\0/uncache-t/x");
        s.sync();
        assert_eq!(s.kept.len(), 3, "an old purge is not applied again");
        // A new worker has nothing to catch up on.
        let mut fresh = Store::new(1 << 20);
        put(&mut fresh, b"h\0/uncache-test");
        fresh.sync();
        assert_eq!(fresh.kept.len(), 1);
    }

    fn put_for(s: &mut Store, cx: &Cx, until: u64, dies: u64) {
        let key: Box<[u8]> = key::<false>(&mut Vec::new(), cx).into();
        let k = Arc::new(Kept {
            wire: vec![b'x'; 4].into(),
            head: 0,
            etag: "".into(),
            until,
            dies,
            refreshing: AtomicBool::new(false),
            refresh: None,
        });
        assert!(s.insert(key, k, 0));
    }

    #[test]
    fn stale_is_sent_inside_its_window_and_the_refresher_gets_a_miss() {
        let mut s = Store::new(1 << 20);
        let c = cx(&[]);
        put_for(&mut s, &c, 100, 130);
        assert!(s.get::<false>(&c, 99).is_some(), "fresh");
        assert!(s.get::<false>(&c, 100).is_some(), "stale, in its window");
        assert!(s.get::<false>(&c, 130).is_none(), "past the window");
        let refresher =
            Cx::from_request::<Fuzz>("GET", "/p?q=1", [], b"", ([0, 0, 0, 0], 0).into()).unwrap();
        assert!(s.get::<false>(&refresher, 110).is_none(), "refresh renders");
        assert!(s.get::<false>(&refresher, 99).is_some(), "fresh is fresh");
        // Past its window, an entry is dropped to make room; a stale one stays.
        let mut s = Store::new(30);
        put_for(&mut s, &c, 100, 130);
        let other =
            Cx::from_request::<Fuzz>("GET", "/o", [], b"", ([1, 1, 1, 1], 1).into()).unwrap();
        let key: Box<[u8]> = key::<false>(&mut Vec::new(), &other).into();
        assert!(
            !s.insert(key.clone(), kept(25, 500), 110),
            "stale one stays"
        );
        assert!(s.insert(key, kept(25, 500), 131), "a dead one goes");
    }

    #[test]
    fn tags_drop_what_is_filed_under_them() {
        let mut s = Store::new(1 << 20);
        for key in [&b"h /t-a"[..], b"h /t-b", b"h /t-c"] {
            assert!(s.insert(key.into(), kept(1, u64::MAX), 0));
        }
        s.tag(b"h /t-a", vec!["t-posts".into(), "t-home".into()]);
        s.tag(b"h /t-b", vec!["t-posts".into()]);
        purge_tag("t-posts");
        s.sync();
        assert_eq!(s.kept.len(), 1);
        assert!(s.kept.contains_key(&b"h /t-c"[..]));
        assert!(s.tags.is_empty(), "the index forgets keys that are gone");
        assert_eq!(s.bytes, size(b"h /t-c", &kept(1, 0)));
    }

    #[test]
    fn draft_mode_is_a_signed_cookie() {
        let mut c = cx(&[]);
        assert!(!c.draft());
        c.enter_draft();
        assert!(c.draft());
        c.exit_draft();
        assert!(!c.draft());
        // A forged one is not it.
        assert!(!cx(&[("cookie", "wisp-draft=1.AAAA")]).draft());
        assert!(!drafting(&cx(&[]), true), "no cookie, no look");
        assert!(!drafting(&cx(&[("cookie", "a=1")]), true));
        assert!(!drafting(&cx(&[("cookie", "wisp-draft=1.AAAA")]), false));
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
            dies: u64::MAX,
            refreshing: AtomicBool::new(false),
            refresh: None,
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
