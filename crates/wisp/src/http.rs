//! HTTP/1.1 server.
//!
//! One task per connection. Each connection owns a `Cx` (which owns the read
//! buffer), a write buffer and an `Out`, all reused across requests, so a
//! warm connection allocates nothing for a typical page. Requests are parsed
//! in place and every complete request in the read buffer is answered
//! before a single write, which gives pipelining for free.
//!
//! Deliberately not here: TLS, HTTP/2, compression, chunked request bodies.
//! A reverse proxy or CDN does those better.

use crate::cx::{Cx, Method, Span, decode};
use crate::{App, Error, Out, Response, dev, rt};
use std::borrow::Cow;
use std::cell::Cell;
use std::future::Future;
use std::net::SocketAddr;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::OnceLock;
use std::task::Poll;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::Instant;

const MAX_HEADERS: usize = 64;
const MAX_HEAD: usize = 16 * 1024;
const MAX_BODY: usize = 1024 * 1024;
const MAX_SEGS: usize = 32;
/// Time allowed to receive one whole request once its first byte arrived.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Time an idle keep-alive connection is kept open.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// A buffer that grew past this for one large message is shrunk afterwards,
/// so memory per idle connection stays bounded.
const KEEP_CAPACITY: usize = 64 * 1024;

const CLIENT_JS: &[u8] = include_bytes!("client/wisp.js");
const CLIENT_JS_ETAG: &str = concat!("\"", env!("CARGO_PKG_VERSION"), "\"");
/// Live reload and the build error dialog. Served and linked only by debug
/// builds, so none of it ships in a release binary's pages.
const DEV_JS: &[u8] = include_bytes!("client/wisp-dev.js");

/// `<link>`/`<script>` tags for `%wisp.head%`. Fixed for the process.
static HEAD_TAGS: OnceLock<String> = OnceLock::new();

/// Thread per core: `threads` single-threaded runtimes, each with its own I/O
/// driver and timers, and this thread accepting connections and handing
/// them out in turn. A connection stays on one thread for its whole life,
/// so the request path never wakes another thread or shares a driver.
/// (A multi-threaded tokio runtime funnels every socket event through one
/// driver; it measured at under half the throughput with cores left idle.)
pub(crate) fn run<A: App>(addr: SocketAddr, threads: usize) -> std::io::Result<()> {
    let listener = std::net::TcpListener::bind(addr)?;
    let workers = (0..threads.max(1))
        .map(|i| {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
            let handle = runtime.handle().clone();
            std::thread::Builder::new().name(format!("wisp-{i}")).spawn(move || runtime.block_on(std::future::pending::<()>()))?;
            Ok(handle)
        })
        .collect::<std::io::Result<Vec<_>>>()?;
    started::<A>(listener.local_addr()?);

    let mut next = 0;
    loop {
        let (stream, peer) = match listener.accept() {
            Ok(c) => c,
            Err(e) => {
                if accept_failed(&e) {
                    std::thread::sleep(Duration::from_millis(50));
                }
                continue;
            }
        };
        let _ = stream.set_nodelay(true);
        if stream.set_nonblocking(true).is_err() {
            continue;
        }
        // Converted on the worker so it registers with that worker's driver.
        workers[next].spawn(async move {
            if let Ok(stream) = TcpStream::from_std(stream) {
                connection::<A>(stream, peer).await;
            }
        });
        next = (next + 1) % workers.len();
    }
}

/// Serves from inside a runtime the caller owns.
pub(crate) async fn serve<A: App>(addr: SocketAddr) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    started::<A>(listener.local_addr()?);
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let _ = stream.set_nodelay(true);
                tokio::spawn(connection::<A>(stream, peer));
            }
            Err(e) => {
                if accept_failed(&e) {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }
}

fn started<A: App>(addr: SocketAddr) {
    HEAD_TAGS.get_or_init(|| {
        let mut s = String::new();
        if let Some(v) = A::CSS {
            s.push_str(&format!("<link rel=\"stylesheet\" href=\"/_app/app.css?v={v}\">"));
        }
        let version = env!("CARGO_PKG_VERSION");
        s.push_str(&format!("<script defer src=\"/_app/wisp.js?v={version}\"></script>"));
        if let Some(port) = dev::events_port() {
            s.push_str(&format!("<script defer src=\"/_app/wisp-dev.js\" data-port=\"{port}\"></script>"));
        }
        s
    });
    // `wisp dev` waits for this exact line to know the app is ready.
    println!("wisp: listening on http://{addr}");
}

/// A client that gave up before we accepted is routine. Anything else (out
/// of file descriptors...) is logged, and the caller should back off
/// instead of spinning: the return value says so.
fn accept_failed(e: &std::io::Error) -> bool {
    use std::io::ErrorKind::{ConnectionAborted, ConnectionReset, Interrupted};
    if matches!(e.kind(), ConnectionAborted | ConnectionReset | Interrupted) {
        return false;
    }
    eprintln!("wisp: accept failed: {e}");
    true
}

enum Parsed {
    /// A complete request is described by `cx`; it occupies `len` bytes.
    Request { len: usize, keep_alive: bool },
    /// More bytes are needed. `need` is the full request size when known.
    Partial { need: usize, expect_continue: bool },
    /// Malformed or refused: answer with this status and close.
    Invalid(u16),
}

async fn connection<A: App>(mut stream: TcpStream, peer: SocketAddr) {
    let mut cx = Cx::new(peer);
    let mut wbuf: Vec<u8> = Vec::with_capacity(16 * 1024);
    let mut out = Out::default();
    let mut partial_since: Option<Instant> = None;
    let mut sent_continue = false;

    loop {
        let mut used = 0;
        let mut need = 0;
        let mut close = false;
        while used < cx.buf.len() {
            match parse(&mut cx, used) {
                Parsed::Request { len, keep_alive } => {
                    respond::<A>(&mut cx, &mut out, &mut wbuf, keep_alive).await;
                    used += len;
                    partial_since = None;
                    sent_continue = false;
                    if !keep_alive {
                        close = true;
                        break;
                    }
                }
                Parsed::Partial { need: n, expect_continue } => {
                    need = n;
                    if expect_continue && !sent_continue {
                        wbuf.extend_from_slice(b"HTTP/1.1 100 Continue\r\n\r\n");
                        sent_continue = true;
                    }
                    break;
                }
                Parsed::Invalid(status) => {
                    simple(&mut wbuf, status, reason(status), Reply { keep_alive: false, head_only: false });
                    close = true;
                    break;
                }
            }
        }

        if !wbuf.is_empty() {
            if stream.write_all(&wbuf).await.is_err() {
                return;
            }
            wbuf.clear();
            if wbuf.capacity() > KEEP_CAPACITY {
                wbuf.shrink_to(KEEP_CAPACITY);
            }
        }
        if close {
            let _ = stream.shutdown().await;
            return;
        }

        cx.buf.drain(..used);
        if cx.buf.is_empty() && cx.buf.capacity() > KEEP_CAPACITY {
            cx.buf.shrink_to(8 * 1024);
        }
        if out.body.capacity() > KEEP_CAPACITY {
            out = Out::default();
        }
        // Reserve the whole body at once when its size is known.
        cx.buf.reserve(need.saturating_sub(cx.buf.len()).max(4096));

        let deadline = if cx.buf.is_empty() {
            Instant::now() + IDLE_TIMEOUT
        } else {
            *partial_since.get_or_insert_with(Instant::now) + REQUEST_TIMEOUT
        };
        match tokio::time::timeout_at(deadline, stream.read_buf(&mut cx.buf)).await {
            Ok(Ok(n)) if n > 0 => {}
            _ => return, // closed, error or too slow
        }
    }
}

/// Parses the request starting at `cx.buf[at..]` and, if it is complete,
/// records it in `cx` as spans.
fn parse(cx: &mut Cx, at: usize) -> Parsed {
    cx.reset();
    let buf = &cx.buf[..];
    let mut raw = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut req = httparse::Request::new(&mut raw);
    let head_len = match req.parse(&buf[at..]) {
        Ok(httparse::Status::Complete(n)) if n <= MAX_HEAD => n,
        Ok(httparse::Status::Partial) if buf.len() - at <= MAX_HEAD => return Parsed::Partial { need: 0, expect_continue: false },
        Ok(_) | Err(httparse::Error::TooManyHeaders) => return Parsed::Invalid(431),
        Err(_) => return Parsed::Invalid(400),
    };

    let mut content_length: Option<usize> = None;
    let mut transfer_encoding = false;
    let mut keep_alive = req.version == Some(1);
    let mut expect_continue = false;
    for h in req.headers.iter() {
        if h.name.eq_ignore_ascii_case("content-length") {
            // Strict: digits only, and repeated headers must agree (smuggling).
            match (parse_decimal(h.value), content_length) {
                (Some(n), None) => content_length = Some(n),
                (Some(n), Some(m)) if n == m => {}
                _ => return Parsed::Invalid(400),
            }
        } else if h.name.eq_ignore_ascii_case("transfer-encoding") {
            transfer_encoding = true;
        } else if h.name.eq_ignore_ascii_case("connection") {
            for token in h.value.split(|&b| b == b',').map(<[u8]>::trim_ascii) {
                if token.eq_ignore_ascii_case(b"close") {
                    keep_alive = false;
                } else if token.eq_ignore_ascii_case(b"keep-alive") {
                    keep_alive = true;
                }
            }
        } else if h.name.eq_ignore_ascii_case("expect") {
            expect_continue = h.value.trim_ascii().eq_ignore_ascii_case(b"100-continue");
        }
    }
    if transfer_encoding {
        // Both framings at once is the classic smuggling vector.
        return Parsed::Invalid(if content_length.is_some() { 400 } else { 501 });
    }
    let len = content_length.unwrap_or(0);
    if len > MAX_BODY {
        return Parsed::Invalid(413);
    }
    let total = head_len + len;
    if buf.len() - at < total {
        return Parsed::Partial { need: total, expect_continue };
    }

    let target = req.path.unwrap_or("");
    let (path, query) = match target.find('?') {
        Some(i) => (&target[..i], &target[i + 1..]),
        None => (target, ""),
    };
    cx.method = Method::parse(req.method.unwrap_or(""));
    cx.path = Span::of(buf, path.as_bytes());
    cx.query = Span::of(buf, query.as_bytes());
    cx.body = Span { start: (at + head_len) as u32, len: len as u32 };
    cx.headers.clear();
    for h in req.headers.iter() {
        cx.headers.push((Span::of(buf, h.name.as_bytes()), Span::of(buf, h.value)));
    }
    Parsed::Request { len: total, keep_alive }
}

async fn respond<A: App>(cx: &mut Cx, out: &mut Out, w: &mut Vec<u8>, keep_alive: bool) {
    let started = std::time::Instant::now();
    let method = cx.method;
    let r = Reply { keep_alive, head_only: method == Method::Head };

    let path = cx.path();
    if !path.starts_with('/') {
        return simple(w, 400, "Bad Request", r);
    }
    if cfg!(debug_assertions) && path.starts_with("/_wisp/") {
        let (status, msg) = dev::endpoint::<A>(method, path, cx.body(), cx.peer());
        return simple(w, status, msg, r);
    }
    if matches!(method, Method::Get | Method::Head) && file::<A>(cx, w, r) {
        return;
    }
    if path.len() > 1 && path.ends_with('/') {
        let trimmed = path.trim_end_matches('/');
        let trimmed = if trimmed.is_empty() { "/" } else { trimmed };
        let query = cx.query_string();
        let location = if query.is_empty() { trimmed.to_string() } else { format!("{trimmed}?{query}") };
        start(w, 308, "text/plain; charset=utf-8", 0, r);
        header(w, "location", &location);
        w.extend_from_slice(b"\r\n");
        return;
    }

    // Route, then turn the matched parameters into spans so `cx` can be
    // handed out mutably.
    let mut segs = [""; MAX_SEGS];
    let route = split(path, &mut segs)
        .and_then(|n| A::route(path, &segs[..n]))
        .map(|(id, raw)| (id, raw.map(|s| Span::of(&cx.buf, s.as_bytes()))));
    out.clear();
    let result = match route {
        Some((id, params)) => {
            cx.set_params(A::PARAMS[id], params);
            catch(A::handle(id, cx, out)).await
        }
        None => Err(Error::new(404, "Not Found")),
    };

    let status = match result {
        Ok(()) => match out.response.take() {
            Some(res) => {
                write_response(w, &res, &cx.out_headers, r);
                res.status
            }
            None => {
                write_page::<A>(w, cx.status, &cx.out_headers, out, r);
                cx.status
            }
        },
        Err(e) if e.status < 400 => {
            // Redirect. Headers set before it (a login cookie) still apply.
            start(w, e.status, "text/plain; charset=utf-8", 0, r);
            if let Some((name, value)) = &e.header {
                header(w, name, value);
            }
            for (name, value) in &cx.out_headers {
                header(w, name, value);
            }
            w.extend_from_slice(b"\r\n");
            e.status
        }
        Err(e) => {
            if e.status >= 500 {
                eprintln!("wisp: {} {}: {e:?}", method.as_str(), cx.path());
            }
            // 5xx details can leak internals; only dev builds show them.
            let message = if e.status >= 500 && !cfg!(debug_assertions) { "The server could not finish this request." } else { e.message() };
            out.clear();
            cx.out_headers.clear();
            let rendered = catch(A::error(route.map(|(id, _)| id), cx, out, e.status, message)).await;
            if rendered.is_err() || out.response.is_some() {
                out.clear();
                rt::default_error(cx, out, e.status, message);
            }
            let extra: Vec<(Cow<'static, str>, String)> = e.header.iter().map(|(n, v)| (Cow::Borrowed(*n), v.clone())).collect();
            write_page::<A>(w, e.status, &extra, out, r);
            e.status
        }
    };

    if cfg!(debug_assertions) {
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        eprintln!("{} {} {status} {ms:.1}ms", method.as_str(), cx.path());
    }
}

/// Runs a handler future, turning a panic into a 500 so one bad request
/// cannot take the connection (or anything else) down with it.
async fn catch<F: Future<Output = crate::Result<()>>>(f: F) -> crate::Result<()> {
    let mut f = std::pin::pin!(f);
    std::future::poll_fn(move |cx| match catch_unwind(AssertUnwindSafe(|| f.as_mut().poll(cx))) {
        Ok(poll) => poll,
        Err(panic) => {
            let msg = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".into());
            Poll::Ready(Err(Error::new(500, format!("panic: {msg}"))))
        }
    })
    .await
}

/// `/a/b` → `["a", "b"]`, `/` → `[]`. `None` when deeper than `MAX_SEGS`.
fn split<'a>(path: &'a str, segs: &mut [&'a str; MAX_SEGS]) -> Option<usize> {
    if path == "/" {
        return Some(0);
    }
    let mut n = 0;
    for s in path[1..].split('/') {
        *segs.get_mut(n)? = s;
        n += 1;
    }
    Some(n)
}

/// Static files: the client script, then embedded assets (release) or
/// files on disk (dev). Returns false if the path is not a file.
fn file<A: App>(cx: &Cx, w: &mut Vec<u8>, r: Reply) -> bool {
    let path = cx.path();
    let versioned = cx.query_string().split('&').any(|kv| kv.starts_with("v="));
    let inm = cx.header("if-none-match");
    if path == "/_app/wisp.js" {
        send_file(w, CLIENT_JS, "js", Some(CLIENT_JS_ETAG), versioned, inm, r);
        return true;
    }
    if cfg!(debug_assertions) && path == "/_app/wisp-dev.js" {
        send_file(w, DEV_JS, "js", None, false, inm, r);
        return true;
    }
    if cfg!(debug_assertions) {
        let Some((bytes, ext)) = dev::read_file(A::ROOT, path) else { return false };
        send_file(w, &bytes, &ext, None, false, inm, r);
        return true;
    }
    let Some(a) = A::asset(path) else { return false };
    send_file(w, a.body, a.ext, Some(a.etag), versioned, inm, r);
    true
}

fn send_file(w: &mut Vec<u8>, body: &[u8], ext: &str, etag: Option<&str>, versioned: bool, if_none_match: Option<&str>, r: Reply) {
    let cache = match etag {
        None => "no-store",
        Some(_) if versioned => "public, max-age=31536000, immutable",
        Some(_) => "public, max-age=0, must-revalidate",
    };
    if let Some(tag) = etag
        && if_none_match == Some(tag)
    {
        status_line(w, 304);
        header(w, "etag", tag);
        header(w, "cache-control", cache);
        date(w);
        if !r.keep_alive {
            w.extend_from_slice(b"connection: close\r\n");
        }
        w.extend_from_slice(b"\r\n");
        return;
    }
    start(w, 200, mime(ext), body.len(), r);
    if let Some(tag) = etag {
        header(w, "etag", tag);
    }
    header(w, "cache-control", cache);
    w.extend_from_slice(b"\r\n");
    if !r.head_only {
        w.extend_from_slice(body);
    }
}

fn write_page<A: App>(w: &mut Vec<u8>, status: u16, extra: &[(Cow<'static, str>, String)], out: &Out, r: Reply) {
    let [s0, s1, s2] = A::shell();
    let tags = HEAD_TAGS.get().map_or("", String::as_str);
    let len = s0.len() + tags.len() + out.head.len() + s1.len() + out.body.len() + s2.len();
    start(w, status, "text/html; charset=utf-8", len, r);
    for (name, value) in extra {
        header(w, name, value);
    }
    w.extend_from_slice(b"\r\n");
    if !r.head_only {
        w.reserve(len);
        for part in [s0, tags, out.head.as_str(), s1, out.body.as_str(), s2] {
            w.extend_from_slice(part.as_bytes());
        }
    }
}

fn write_response(w: &mut Vec<u8>, res: &Response, extra: &[(Cow<'static, str>, String)], r: Reply) {
    start(w, res.status, &res.content_type, res.body.len(), r);
    for (name, value) in res.headers.iter().chain(extra) {
        header(w, name, value);
    }
    w.extend_from_slice(b"\r\n");
    if !r.head_only {
        w.extend_from_slice(&res.body);
    }
}

#[derive(Clone, Copy)]
struct Reply {
    keep_alive: bool,
    head_only: bool,
}

/// Status line and the headers every response has. The caller adds its own
/// headers and the blank line.
fn start(w: &mut Vec<u8>, status: u16, content_type: &str, len: usize, r: Reply) {
    status_line(w, status);
    header(w, "content-type", content_type);
    w.extend_from_slice(b"content-length: ");
    push_decimal(w, len as u64);
    w.extend_from_slice(b"\r\n");
    date(w);
    if !r.keep_alive {
        w.extend_from_slice(b"connection: close\r\n");
    }
}

fn simple(w: &mut Vec<u8>, status: u16, body: &str, r: Reply) {
    start(w, status, "text/plain; charset=utf-8", body.len(), r);
    w.extend_from_slice(b"\r\n");
    if !r.head_only {
        w.extend_from_slice(body.as_bytes());
    }
}

fn status_line(w: &mut Vec<u8>, status: u16) {
    w.extend_from_slice(b"HTTP/1.1 ");
    push_decimal(w, status as u64);
    w.push(b' ');
    w.extend_from_slice(reason(status).as_bytes());
    w.extend_from_slice(b"\r\n");
}

fn header(w: &mut Vec<u8>, name: &str, value: &str) {
    w.extend_from_slice(name.as_bytes());
    w.extend_from_slice(b": ");
    w.extend_from_slice(value.as_bytes());
    w.extend_from_slice(b"\r\n");
}

fn push_decimal(w: &mut Vec<u8>, mut n: u64) {
    let mut digits = [0u8; 20];
    let mut i = digits.len();
    loop {
        i -= 1;
        digits[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    w.extend_from_slice(&digits[i..]);
}

/// Digits only, no sign or whitespace, no overflow.
fn parse_decimal(s: &[u8]) -> Option<usize> {
    if s.is_empty() || s.len() > 19 {
        return None;
    }
    s.iter().try_fold(0usize, |n, &b| b.is_ascii_digit().then(|| n * 10 + (b - b'0') as usize))
}

thread_local! {
    /// (unix second, formatted date) — reformatted at most once a second.
    static DATE: Cell<(u64, [u8; 29])> = const { Cell::new((u64::MAX, [0; 29])) };
}

fn date(w: &mut Vec<u8>) {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let text = DATE.with(|c| {
        let (secs, text) = c.get();
        if secs == now {
            return text;
        }
        let text = http_date(now);
        c.set((now, text));
        text
    });
    w.extend_from_slice(b"date: ");
    w.extend_from_slice(&text);
    w.extend_from_slice(b"\r\n");
}

/// IMF-fixdate, e.g. `Sun, 06 Nov 1994 08:49:37 GMT`.
fn http_date(secs: u64) -> [u8; 29] {
    const DAYS: [&[u8; 3]; 7] = [b"Thu", b"Fri", b"Sat", b"Sun", b"Mon", b"Tue", b"Wed"];
    const MONTHS: [&[u8; 3]; 12] = [b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct", b"Nov", b"Dec"];
    let days = secs / 86_400;
    let rem = secs % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + (month <= 2) as i64;

    let two = |n: u64| [b'0' + (n / 10) as u8, b'0' + (n % 10) as u8];
    let mut out = [0u8; 29];
    out[..3].copy_from_slice(DAYS[(days % 7) as usize]);
    out[3..5].copy_from_slice(b", ");
    out[5..7].copy_from_slice(&two(day as u64));
    out[7] = b' ';
    out[8..11].copy_from_slice(MONTHS[month as usize - 1]);
    out[11] = b' ';
    let y = year as u64;
    out[12..14].copy_from_slice(&two(y / 100 % 100));
    out[14..16].copy_from_slice(&two(y % 100));
    out[16] = b' ';
    out[17..19].copy_from_slice(&two(rem / 3600));
    out[19] = b':';
    out[20..22].copy_from_slice(&two(rem / 60 % 60));
    out[22] = b':';
    out[23..25].copy_from_slice(&two(rem % 60));
    out[25..29].copy_from_slice(b" GMT");
    out
}

pub(crate) fn reason(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        410 => "Gone",
        413 => "Content Too Large",
        415 => "Unsupported Media Type",
        422 => "Unprocessable Content",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "",
    }
}

pub(crate) fn mime(ext: &str) -> &'static str {
    match ext {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "webmanifest" => "application/manifest+json",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        "wasm" => "application/wasm",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// Percent-decoded static file path, or `None` if it could escape `static/`.
pub(crate) fn safe_relative_path(path: &str) -> Option<String> {
    let decoded = decode(path.as_bytes(), false);
    let rel = decoded.strip_prefix('/')?;
    let bad = |s: &str| s.is_empty() || s == "." || s == ".." || s.contains(['\\', ':', '\0']);
    if rel.split('/').any(bad) {
        return None;
    }
    Some(rel.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(&http_date(784_111_777), b"Sun, 06 Nov 1994 08:49:37 GMT");
        assert_eq!(&http_date(0), b"Thu, 01 Jan 1970 00:00:00 GMT");
        assert_eq!(&http_date(951_782_400), b"Tue, 29 Feb 2000 00:00:00 GMT");
        assert_eq!(&http_date(4_102_444_799), b"Thu, 31 Dec 2099 23:59:59 GMT");
    }

    #[test]
    fn decimals() {
        let mut v = Vec::new();
        push_decimal(&mut v, 0);
        v.push(b' ');
        push_decimal(&mut v, 18_446_744_073_709_551_615);
        assert_eq!(v, b"0 18446744073709551615");
        assert_eq!(parse_decimal(b"1234"), Some(1234));
        assert_eq!(parse_decimal(b""), None);
        assert_eq!(parse_decimal(b"+1"), None);
        assert_eq!(parse_decimal(b"1 "), None);
        assert_eq!(parse_decimal(b"99999999999999999999"), None);
    }

    #[test]
    fn paths() {
        let mut segs = [""; MAX_SEGS];
        assert_eq!(split("/", &mut segs), Some(0));
        assert_eq!(split("/a/b", &mut segs), Some(2));
        assert_eq!(&segs[..2], ["a", "b"]);
        assert_eq!(split(&"/x".repeat(40), &mut segs), None);
        assert_eq!(safe_relative_path("/img/a%20b.png").as_deref(), Some("img/a b.png"));
        for bad in ["/../secret", "/a/%2e%2e/b", "/a//b", "/", "/C:/x", "/a\\b", "/a/"] {
            assert_eq!(safe_relative_path(bad), None, "{bad}");
        }
    }
}
