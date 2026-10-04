//! The reply and its body, and how it is written to the wire.

use super::*;

/// A response, decided but not yet written: what every host sends, the
/// built-in server, [`handle`], tower. The host adds `content-length`,
/// `date` and `connection` as its protocol needs.
pub struct Reply {
    /// The HTTP status code.
    pub status: u16,
    /// `content-type` first, when there is a body.
    pub headers: Vec<(Cow<'static, str>, Cow<'static, str>)>,
    /// The body.
    pub body: Body,
}

/// A [`Reply`]'s body.
pub enum Body {
    /// Bytes the reply owns.
    Bytes(Vec<u8>),
    /// Bytes compiled into the binary.
    Static(&'static [u8]),
    /// The page in the request's `Out`, inside the app's shell. Only the
    /// built-in server sees it; [`handle`] renders it to `Bytes`.
    #[doc(hidden)]
    Page,
    /// A response made before (a baked page, or one `CACHE` kept), whose
    /// head and body are sent as they are. [`handle`] turns it into headers
    /// and `Static` or `Bytes`.
    #[doc(hidden)]
    Made(crate::bake::Made),
    /// Chunks as a [`Response::stream`] makes them, until the sender is dropped.
    Stream(mpsc::Receiver<Vec<u8>>),
    /// A [`Response::websocket`]: only the built-in server upgrades;
    /// [`handle`] answers it with a 501.
    #[doc(hidden)]
    WebSocket(crate::ws::Upgrade),
}

impl Default for Reply {
    fn default() -> Reply {
        Reply {
            status: 200,
            headers: Vec::new(),
            body: Body::Static(b""),
        }
    }
}

impl Reply {
    pub(crate) fn plain(status: u16) -> Reply {
        let mut r = Reply::default();
        r.set_plain(status, reason(status));
        r
    }

    /// Starts over as a response of `content_type`. The headers keep their
    /// capacity, so a connection's `Reply` allocates nothing once warm.
    pub(crate) fn set(&mut self, status: u16, content_type: &'static str, body: Body) {
        self.status = status;
        self.headers.clear();
        self.headers
            .push((Cow::Borrowed("content-type"), Cow::Borrowed(content_type)));
        if matches!(body, Body::Page)
            && let Some(policy) = crate::csp::header()
        {
            self.headers.push((
                Cow::Borrowed("content-security-policy"),
                Cow::Borrowed(policy),
            ));
        }
        self.body = body;
    }

    pub(crate) fn set_plain(&mut self, status: u16, text: &'static str) {
        self.set(
            status,
            "text/plain; charset=utf-8",
            Body::Static(text.as_bytes()),
        );
    }

    pub(super) fn add(&mut self, headers: Vec<(Cow<'static, str>, String)>) {
        self.headers
            .extend(headers.into_iter().map(|(n, v)| (n, Cow::Owned(v))));
    }

    /// The upgrade of a [`Response::websocket`], taken out of the body.
    pub(super) fn take_websocket(&mut self) -> Option<crate::ws::Upgrade> {
        if !matches!(self.body, Body::WebSocket(_)) {
            return None;
        }
        match std::mem::replace(&mut self.body, Body::Static(b"")) {
            Body::WebSocket(upgrade) => Some(upgrade),
            _ => None,
        }
    }

    /// The first header called `name`.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| &**v)
    }

    /// The body, when it is not a stream.
    pub fn bytes(&self) -> &[u8] {
        match &self.body {
            Body::Bytes(b) => b,
            Body::Static(b) => b,
            Body::Made(m) => m.body(),
            Body::Page | Body::Stream(_) | Body::WebSocket(_) => b"",
        }
    }

    /// The body as text; empty if it is not UTF-8 or is a stream.
    pub fn text(&self) -> &str {
        std::str::from_utf8(self.bytes()).unwrap_or("")
    }

    /// The body read from JSON, for tests: `app.get("/api/notes").json::<Vec<Note>>()`,
    /// or `json::<wisp::Value>()` for any JSON. Panics, showing the body,
    /// if it is not a `T`.
    pub fn json<T: crate::FromJson>(&self) -> T {
        match crate::from_json(self.bytes()) {
            Ok(v) => v,
            Err(e) => panic!(
                "the body is not a {}: {e:?}\n{}",
                std::any::type_name::<T>(),
                self.text()
            ),
        }
    }
}

/// A request for [`handle`], from a host other than the built-in server,
/// or from a test.
pub struct Request {
    /// The method: `"GET"`, `"POST"`.
    pub method: String,
    /// Path and query: `/posts?page=2`.
    pub target: String,
    /// The request headers, as `(name, value)`.
    pub headers: Vec<(String, String)>,
    /// The request body.
    pub body: Vec<u8>,
    /// The client's address. Loopback unless set.
    pub peer: SocketAddr,
}

impl Request {
    /// A request with `method` and `target` (path and query), no headers and no body.
    pub fn new(method: &str, target: &str) -> Request {
        Request {
            method: method.into(),
            target: target.into(),
            headers: Vec::new(),
            body: Vec::new(),
            peer: SocketAddr::from(([127, 0, 0, 1], 0)),
        }
    }

    /// Adds a request header.
    pub fn header(&mut self, name: &str, value: &str) {
        self.headers.push((name.into(), value.into()));
    }
}

/// Whether `name` is framing, which the host writes itself: an app's own
/// `content-length` or `transfer-encoding` (copied from another server's
/// response, say) would contradict it, and the client would read the next
/// response wrong. Only an answer to HEAD, which has no body to count, may
/// give its length.
pub(super) fn framing(name: &str, head: bool) -> bool {
    match name.len() {
        14 => !head && name.eq_ignore_ascii_case("content-length"),
        17 => name.eq_ignore_ascii_case("transfer-encoding"),
        _ => false,
    }
}

/// A status whose response has no body (RFC 9110 §8.6, §15): informational,
/// 204, 205 and 304. A body the app gave one is dropped rather than sent
/// where the client does not expect it. None has a `content-length` but
/// 205, which says `0` (§15.3.6) so a client does not wait for content.
pub(super) fn bodiless(status: u16) -> bool {
    status < 200 || matches!(status, 204 | 205 | 304)
}

/// A response whose body is still being made, for the connection to send.
pub(super) struct Streamed {
    pub(super) body: mpsc::Receiver<Vec<u8>>,
    /// Framed in chunks (HTTP/1.1); otherwise the body ends when the
    /// connection closes.
    pub(super) chunked: bool,
    /// The connection closes after it.
    pub(super) close: bool,
}

/// Writes `reply` as HTTP/1.1 and leaves it empty. A streamed body is
/// returned for the connection to send as it comes.
#[inline(always)]
pub(super) fn serialize<A: App, const OBS: bool>(
    w: &mut Vec<u8>,
    reply: &mut Reply,
    out: &mut Out,
    http11: bool,
    keep_alive: bool,
    head_only: bool,
) -> Option<Streamed> {
    emit::<A, OBS, false>(w, reply, out, http11, keep_alive, head_only, |_, _| {})
}

/// [`serialize`] for HTTP/2: the head goes to `sink` as fields, `:status`
/// first, and only the body to `w`.
#[cfg(feature = "h2")]
pub(crate) fn serialize_h2<A: App>(
    w: &mut Vec<u8>,
    reply: &mut Reply,
    out: &mut Out,
    head_only: bool,
    sink: impl FnMut(&str, &str),
) -> Option<mpsc::Receiver<Vec<u8>>> {
    emit::<A, true, true>(w, reply, out, true, true, head_only, sink).map(|s| s.body)
}

/// [`serialize`], and with `H2` [`serialize_h2`]: the head's fields go to
/// `sink` rather than to `w`.
#[inline(never)]
pub(super) fn emit<A: App, const OBS: bool, const H2: bool>(
    w: &mut Vec<u8>,
    reply: &mut Reply,
    out: &mut Out,
    http11: bool,
    keep_alive: bool,
    head_only: bool,
    mut sink: impl FnMut(&str, &str),
) -> Option<Streamed> {
    let bodiless = bodiless(reply.status);
    let stream = matches!(reply.body, Body::Stream(_)) && !bodiless;
    // HTTP/1.0 has no chunks: a streamed body ends with the connection.
    let chunked = stream && http11;
    let keep_alive = keep_alive && (!stream || chunked || head_only);
    let page = matches!(reply.body, Body::Page);
    let parts = page.then(|| parts::<A>(&out.live, out.lang, &out.head, &mut out.body));
    let len = match &parts {
        Some(parts) => parts.iter().map(|p| p.len()).sum(),
        None => reply.bytes().len(),
    };
    if OBS && out.obs.is_some() {
        let sent = if head_only || bodiless || stream {
            0
        } else {
            len
        };
        crate::obs::tag(&out.obs, reply);
        crate::obs::finish(&mut out.obs, reply.status, sent);
    }

    // See `framing`.
    let own_length = head_only && reply.header("content-length").is_some();
    let made = matches!(reply.body, Body::Made(_));
    let length = !made && !chunked && !stream && !bodiless && !own_length;
    if H2 {
        h2_head(reply, length.then_some(len), &mut sink);
    } else if let Body::Made(m) = &reply.body {
        w.extend_from_slice(m.head()); // status line and length included
    } else {
        match status_line(reply.status) {
            Some(line) => w.extend_from_slice(line),
            None => {
                w.extend_from_slice(b"HTTP/1.1 ");
                push_decimal(w, reply.status.into());
                w.extend_from_slice(b" \r\n");
            }
        }
        if chunked {
            w.extend_from_slice(b"transfer-encoding: chunked\r\n");
        }
    }
    if !H2 {
        length_and_date(w, length.then_some(len)); // a 205's is in its status line
        if !keep_alive {
            w.extend_from_slice(b"connection: close\r\n");
        } else if !http11 {
            // HTTP/1.0 closes after each response unless told otherwise.
            w.extend_from_slice(b"connection: keep-alive\r\n");
        }
        fields(reply, head_only, |h| header(w, h));
        w.extend_from_slice(b"\r\n");
    } else {
        fields(reply, head_only, |(n, v)| {
            if valid_header(n, v) {
                sink(n, v)
            }
        });
    }

    let body = std::mem::replace(&mut reply.body, Body::Static(b""));
    reply.headers.clear();
    let send = !head_only && !bodiless;
    match body {
        Body::Bytes(b) => {
            if send {
                w.extend_from_slice(&b);
            }
            recycle(b);
        }
        Body::Static(b) if send => w.extend_from_slice(b),
        Body::Made(m) if send => w.extend_from_slice(m.body()),
        Body::Page if send => {
            w.reserve(len);
            for part in parts.into_iter().flatten() {
                w.extend_from_slice(part.as_bytes());
            }
        }
        Body::Stream(body) if send => {
            return Some(Streamed {
                body,
                chunked,
                close: !keep_alive,
            });
        }
        // None to send: HEAD, 204, 304, or a 101's upgrade, taken out before.
        _ => {}
    }
    None
}

/// The head of `reply` as HTTP/2 fields, all but the app's own: `:status`,
/// then a made answer's fixed fields or the `content-length` of `length`
/// (and a 205's), then `date`. Framing (`transfer-encoding`) is left out:
/// HTTP/2 has none.
#[cfg(feature = "h2")]
#[cold]
pub(super) fn h2_head(reply: &Reply, length: Option<usize>, sink: &mut impl FnMut(&str, &str)) {
    let decimal = |n: u64| {
        let mut buf = [0u8; 20];
        let at = crate::digits(&mut buf, 20, n);
        // Digits are ASCII.
        String::from_utf8_lossy(&buf[at..]).into_owned()
    };
    if let Body::Made(m) = &reply.body {
        let head = std::str::from_utf8(m.head()).unwrap_or("");
        let mut lines = head.split("\r\n");
        let status = lines.next().unwrap_or("").get(9..12).unwrap_or("500");
        sink(":status", status);
        for line in lines {
            if let Some((n, v)) = line.split_once(':') {
                sink(n.trim(), v.trim());
            }
        }
    } else {
        sink(":status", &decimal(reply.status.into()));
        if reply.status == 205 {
            sink("content-length", "0");
        }
        if let Some(n) = length {
            sink("content-length", &decimal(n as u64));
        }
    }
    let date = date_line();
    sink("date", std::str::from_utf8(&date[6..35]).unwrap_or(""));
}

/// Without HTTP/2 nothing asks for the head as fields.
#[cfg(not(feature = "h2"))]
#[inline(always)]
pub(super) fn h2_head(_: &Reply, _: Option<usize>, _: &mut impl FnMut(&str, &str)) {}

/// A header field as a reply holds it.
type Field = (Cow<'static, str>, Cow<'static, str>);

/// The app's own header fields of `reply` that go out, each to `sink`: all
/// but framing (see `framing`). The sink leaves out any that would split
/// the response: HTTP/1 writes the rest to the wire (`header`), HTTP/2 to
/// its encoder. The field comes whole, so the HTTP/1 sink can keep its
/// check of `'static` pairs.
#[inline(always)]
pub(super) fn fields(reply: &Reply, head_only: bool, mut sink: impl FnMut(&Field)) {
    for h in &reply.headers {
        if !framing(&h.0, head_only) {
            sink(h);
        }
    }
}

/// The parts of a page, in order: the shell around the tags for
/// `%wisp.head%`, the page's head, and its body, which its browser code
/// ends. Once a page.
pub(crate) fn page<A: App>(out: &mut Out) -> [&str; 8] {
    parts::<A>(&out.live, out.lang, &out.head, &mut out.body)
}

/// [`page`] of an `Out`'s fields, leaving the others free.
pub(super) fn parts<'a, A: App>(
    live: &crate::live::Live,
    lang: u8,
    head: &'a str,
    body: &'a mut String,
) -> [&'a str; 8] {
    live.tail(body, lang);
    let [s0, s1, s2] = A::shell();
    let tags = HEAD_TAGS.get().map_or("", String::as_str);
    // `<html lang="…">` says the request's locale, in an app with some.
    let lang = A::HTML_LANGS.get(lang as usize).copied().unwrap_or("");
    let (a, b) = match (!lang.is_empty())
        .then(|| crate::i18n::lang_value(s0))
        .flatten()
    {
        Some((at, end)) => ((&s0[..at], lang), &s0[end..]),
        None => ((s0, ""), ""),
    };
    [a.0, a.1, b, tags, head, s1, body, s2]
}

/// A header the app set with a line break or NUL in it (through `Response`'s
/// public fields, which nothing checks) would split the response: it is
/// left out. A pair of `'static` strings (the code's own, as
/// `content-type: text/plain` is) is checked once a thread while it is one
/// of the last few such pairs: such a string never changes, so its address
/// and length say it is the same.
#[inline(always)]
pub(super) fn header(w: &mut Vec<u8>, (name, value): &Field) {
    /// The pairs a thread keeps checked: a response's few static headers.
    const KEPT: usize = 4;
    thread_local! {
        static VALID: [Cell<[usize; 4]>; KEPT] = const { [const { Cell::new([0; 4]) }; KEPT] };
        static NEXT: Cell<usize> = const { Cell::new(0) };
    }
    if name == "content-type"
        && let Some(line) = type_line(value)
    {
        w.extend_from_slice(line);
        return;
    }
    let key = match (name, value) {
        (Cow::Borrowed(n), Cow::Borrowed(v)) => {
            Some([n.as_ptr() as usize, n.len(), v.as_ptr() as usize, v.len()])
        }
        _ => None,
    };
    if !key.is_some_and(|k| VALID.with(|v| v.iter().any(|c| c.get() == k))) {
        if !valid_header(name, value) {
            return;
        }
        if let Some(k) = key {
            let next = NEXT.get();
            VALID.with(|v| v[next].set(k));
            NEXT.set((next + 1) % KEPT);
        }
    }
    w.extend_from_slice(name.as_bytes());
    w.extend_from_slice(b": ");
    w.extend_from_slice(value.as_bytes());
    w.extend_from_slice(b"\r\n");
}

/// The `content-type` line of the types most responses have, written in
/// one piece.
pub(super) fn type_line(value: &str) -> Option<&'static [u8]> {
    Some(match value {
        "text/plain; charset=utf-8" => b"content-type: text/plain; charset=utf-8\r\n",
        "text/html; charset=utf-8" => b"content-type: text/html; charset=utf-8\r\n",
        "application/json" => b"content-type: application/json\r\n",
        _ => return None,
    })
}

/// `n` in lowercase hex, as few digits as it takes.
pub(super) fn push_hex(w: &mut Vec<u8>, n: u64) {
    let digits = (16 - n.leading_zeros() as usize / 4).max(1);
    for k in (0..digits).rev() {
        w.push(b"0123456789abcdef"[(n >> (4 * k) & 15) as usize]);
    }
}

pub(crate) fn push_decimal(w: &mut Vec<u8>, n: u64) {
    let mut buf = [0u8; 20];
    let start = crate::digits(&mut buf, 20, n);
    w.extend_from_slice(&buf[start..]);
}

/// Digits only, no sign or whitespace, no overflow.
pub(super) fn parse_decimal(s: &[u8]) -> Option<usize> {
    if s.is_empty() || s.len() > 19 {
        return None;
    }
    s.iter().try_fold(0usize, |n, &b| {
        let digit = b.checked_sub(b'0').filter(|&d| d < 10)?;
        n.checked_mul(10)?.checked_add(digit as usize)
    })
}

/// The statuses Wisp names: the reason phrase of each, and its whole
/// status line, written in one piece.
macro_rules! statuses {
    ($($code:literal $reason:literal,)*) => {
        pub(crate) fn reason(status: u16) -> &'static str {
            match status {
                $($code => $reason,)*
                205 => "Reset Content",
                _ => "",
            }
        }

        /// `HTTP/1.1 200 OK\r\n`; `None` for a status not named here. A
        /// 205's says its content is empty (RFC 9110 §15.3.6), which costs
        /// the other statuses nothing.
        #[inline(always)]
        fn status_line(status: u16) -> Option<&'static [u8]> {
            Some(match status {
                $($code => concat!("HTTP/1.1 ", $code, " ", $reason, "\r\n").as_bytes(),)*
                205 => b"HTTP/1.1 205 Reset Content\r\ncontent-length: 0\r\n",
                _ => return None,
            })
        }
    };
}

statuses! {
    100 "Continue",
    101 "Switching Protocols",
    200 "OK",
    201 "Created",
    202 "Accepted",
    204 "No Content",
    301 "Moved Permanently",
    302 "Found",
    303 "See Other",
    304 "Not Modified",
    307 "Temporary Redirect",
    308 "Permanent Redirect",
    400 "Bad Request",
    401 "Unauthorized",
    403 "Forbidden",
    404 "Not Found",
    405 "Method Not Allowed",
    409 "Conflict",
    410 "Gone",
    413 "Content Too Large",
    415 "Unsupported Media Type",
    422 "Unprocessable Content",
    426 "Upgrade Required",
    429 "Too Many Requests",
    431 "Request Header Fields Too Large",
    500 "Internal Server Error",
    501 "Not Implemented",
    502 "Bad Gateway",
    503 "Service Unavailable",
    504 "Gateway Timeout",
}

/// The error page's title: the status's name, or which side failed.
pub(crate) fn title(status: u16) -> &'static str {
    match reason(status) {
        "" if status >= 500 => "Server Error",
        "" => "Request Failed",
        r => r,
    }
}

/// One sentence on what a status means for the person reading the page, for
/// an error that carries nothing more specific.
pub(crate) fn sentence(status: u16) -> &'static str {
    match status {
        400 => "The request could not be understood.",
        401 => "Sign in to see this page.",
        403 => "You do not have access to this page.",
        404 => "There is nothing at this address.",
        405 => "This address does not take that kind of request.",
        409 => "The request conflicts with a change made since.",
        410 => "This page has been removed.",
        413 => "The request is too large.",
        415 => "The request is in a format this address does not take.",
        422 => "The request could not be processed.",
        429 => "Too many requests. Wait a moment, then try again.",
        431 => "The request's headers are too large.",
        501 => "The server does not support this request.",
        502 => "The server got a bad answer from a server behind it.",
        503 => "The server cannot take requests right now. Try again in a moment.",
        504 => "A server behind this one took too long to answer.",
        s if s >= 500 => "The server could not finish this request.",
        _ => "The request could not be completed.",
    }
}
