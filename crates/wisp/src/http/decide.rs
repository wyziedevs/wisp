//! Deciding an answer: the driver path, the decider, hooks and errors.

use super::*;

/// What the epoll driver received for a connection and leaves to its
/// future (see [`on_driver`]): the buffers, how much of `cx.wire.buf` is
/// answered, and the request after that when the driver got that far with
/// it (see [`Ahead`]).
pub(crate) struct Handed {
    pub(super) held: Holding,
    pub(super) at: usize,
    pub(super) ahead: Option<Ahead>,
}

/// The buffers of what is handed, or the request deciding in them: one the
/// build said never waits, which did. The connection's future goes on
/// polling it, so what it did so far is never lost.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(super) enum Holding {
    Ready(Box<Buffers>),
    Deciding(Deciding),
}

/// A decider (see [`decider`]) with a request under way.
pub(super) type Deciding = std::pin::Pin<Box<dyn Future<Output = ()> + Send>>;

impl Handed {
    #[cfg(target_os = "linux")]
    pub(super) fn new(held: Holding, at: usize, ahead: Option<Ahead>) -> Handed {
        Handed { held, at, ahead }
    }

    /// What is still to send, which the driver sends before it hands over.
    /// A request still deciding has the answers before it: they go with
    /// its own.
    #[cfg(target_os = "linux")]
    pub(crate) fn wbuf(&mut self) -> Option<&mut Vec<u8>> {
        match &mut self.held {
            Holding::Ready(b) => Some(&mut b.wbuf),
            Holding::Deciding(_) => None,
        }
    }
}

/// A request [`on_driver`] parsed but leaves to the connection's future,
/// whole: `parse` is not run on it again, which would read a chunked body
/// it already moved over its framing. Its route is in `routed` once the
/// driver found it (its params in `cx`). And whether it is decided too:
/// its reply is in `reply` (a stream, a WebSocket), or is still being
/// decided (`Holding::Deciding`).
pub(super) type Ahead = (Req, bool);

/// Whether some route of `A` answers without waiting, so [`on_driver`] can
/// help. It is not used with logs, metrics or traces on (`obs`): the
/// connection's future has them, and a request pays nothing for them
/// when they are off.
#[cfg(target_os = "linux")]
pub(super) fn answers<A: App>() -> bool {
    A::NOT_FOUND_NOW || A::ROUTES.iter().any(|r| r.now)
}

/// Answers on the epoll driver, without the connection's future, the
/// requests that came for connection `token` while its future waited for
/// them holding nothing: those to routes the build found never wait
/// (`RouteFacts::now`), whole, that keep the connection open. True when that was
/// all of them; false when the future takes over, with what is left handed
/// to it.
///
/// It saves what the future's way costs a request: polling the
/// connection's future and its state, and taking and giving back buffers
/// across it. The work is the same: [`parse`], [`decide`], [`serialize`].
/// A route that waits after all is no failure, only slower: its request,
/// still deciding, goes on in the future.
#[cfg(target_os = "linux")]
pub(crate) fn on_driver<A: App>(token: u64) -> bool {
    use crate::epoll::{self, Got};
    if stopping() {
        return false;
    }
    let Some((id, peer)) = epoll::free(token) else {
        return false;
    };
    let mut b = DRIVER.take().unwrap_or_else(|| take_buffers(peer));
    b.cx.wire.peer = peer;
    let mut got = epoll::receive(id, &mut b.cx.wire.buf);
    loop {
        let more = match got {
            Got::Bytes(more) => more,
            Got::Nothing => break,
            Got::End => {
                park(b);
                return false;
            }
        };
        b = match answer_whole::<A>(b) {
            Ok(b) => b,
            Err(h) => {
                epoll::hand(id, h);
                return false;
            }
        };
        b.cx.wire.buf.clear();
        // Sent, and the next: in one go on the worker. Something answered
        // (all of it), so the connection waits from now on.
        let deadline = policy::idle_deadline(seconds());
        got = epoll::next(id, &mut b.wbuf, &mut b.cx.wire.buf, more, deadline);
    }
    park(b);
    true
}

#[cfg(target_os = "linux")]
thread_local! {
    /// The buffers [`on_driver`] answers in, kept for its next connection:
    /// its requests are each reset as answered, so only what came and what
    /// went is cleared between.
    static DRIVER: Cell<Option<Box<Buffers>>> = const { Cell::new(None) };
}

/// `b` back to [`DRIVER`].
#[cfg(target_os = "linux")]
pub(super) fn park(mut b: Box<Buffers>) {
    reset_buffers(&mut b);
    DRIVER.set(Some(b));
}

/// [`on_driver`]'s requests in `b.cx.wire.buf`, answered into `b.wbuf`:
/// the buffers when that was all of them, else what is left for the
/// connection's future.
#[cfg(target_os = "linux")]
pub(super) fn answer_whole<A: App>(mut b: Box<Buffers>) -> Result<Box<Buffers>, Handed> {
    let mut at = 0;
    while at < b.cx.wire.buf.len() && b.wbuf.len() < KEEP_CAPACITY {
        let Parsed::Request(mut req) = parse::<A>(&mut b.cx, at, true) else {
            break;
        };
        if !policy::keeps_open(req.keep_alive, stopping()) {
            return Err(Handed::new(Holding::Ready(b), at, Some((req, false))));
        }
        let route = req.routed.unwrap_or_else(|| route::<A>(&mut b.cx));
        req.routed = Some(route);
        if !route.map_or(A::NOT_FOUND_NOW, |r| A::ROUTES[r].now) {
            return Err(Handed::new(Holding::Ready(b), at, Some((req, false))));
        }
        let job = match route.is_some_and(|r| A::ROUTES[r].sync & b.cx.method.bit() != 0) {
            true => {
                let Buffers { cx, out, reply, .. } = &mut *b;
                decide_now::<A>(cx, out, reply, route)
            }
            false => Some(Job::Decide(route)),
        };
        if let Some(job) = job {
            b = match at_once::<A>(b, job) {
                Ok(b) => b,
                Err(f) => return Err(Handed::new(Holding::Deciding(f), at, Some((req, true)))),
            };
        }
        if matches!(b.reply.body, Body::Stream(_) | Body::WebSocket(_)) {
            return Err(Handed::new(Holding::Ready(b), at, Some((req, true))));
        }
        let Buffers {
            cx,
            wbuf,
            out,
            reply,
        } = &mut *b;
        serialize::<A, false>(
            wbuf,
            reply,
            out,
            cx.wire.http11,
            true,
            cx.method == Method::Head,
        );
        cx.reset();
        at += req.len;
    }
    if at < b.cx.wire.buf.len() {
        return Err(Handed::new(Holding::Ready(b), at, None));
    }
    Ok(b)
}

#[cfg(target_os = "linux")]
thread_local! {
    /// The decider of [`on_driver`]'s requests on this thread. A thread
    /// drives one app's server (its workers are its own), so it is that app's.
    static DECIDER: Cell<Option<Deciding>> = const { Cell::new(None) };
    /// A request for the decider, and what is left to do for it.
    static INBOX: Cell<Option<(Box<Buffers>, Job)>> = const { Cell::new(None) };
}

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    /// The buffers of the request a decider just decided, taken at once
    /// by what polled it.
    static OUTBOX: Cell<Option<Box<Buffers>>> = const { Cell::new(None) };
}

/// [`decide`] for the driver, as one future a thread keeps: each request
/// comes in through [`INBOX`], its buffers go out through [`OUTBOX`] once
/// it is decided, and no future is made or moved for it. A request that
/// waits keeps its decider, which the connection's future polls on, and
/// the thread makes another.
#[cfg(target_os = "linux")]
pub(super) async fn decider<A: App>() {
    loop {
        let (mut b, job) =
            std::future::poll_fn(|_| INBOX.take().map_or(Poll::Pending, Poll::Ready)).await;
        {
            let Buffers { cx, out, reply, .. } = &mut *b;
            match job {
                Job::Decide(route) => decide::<A>(cx, out, reply, Some(route)).await,
                Job::Page(f) => {
                    render_error::<A>(f.route, cx, out, f.page).await;
                    answered(cx, reply, f.started, f.failure);
                    tag::<A>(cx, reply);
                }
            }
        }
        OUTBOX.set(Some(b));
    }
}

/// What the decider is to do for a request.
#[cfg(target_os = "linux")]
pub(super) enum Job {
    /// All of [`decide`], routed to this.
    Decide(Option<usize>),
    /// The error page of what [`decide_now`] decided.
    Page(Failed),
}

/// A request [`decide_now`] decided but for its error page, which is
/// rendered in a future: an error page may wait.
#[cfg(target_os = "linux")]
pub(super) struct Failed {
    pub(super) route: Option<usize>,
    pub(super) page: (u16, Cow<'static, str>),
    pub(super) failure: Option<String>,
    pub(super) started: Option<Instant>,
}

/// [`decide`], for an arm of the request's route that [`App::handle_now`]
/// answers: with no future, unless for an error page. What is left for the
/// decider, if anything: all of it when the arm turns out to have no sync
/// form, as nothing was done that `decide` does not do again.
#[cfg(target_os = "linux")]
pub(super) fn decide_now<A: App>(
    cx: &mut Cx,
    out: &mut Out,
    reply: &mut Reply,
    route: Option<usize>,
) -> Option<Job> {
    if crate::settings().request_id {
        cx.request_id();
    }
    if !before_routes::<A>(cx, route, reply) {
        let timed = crate::settings().timed;
        let started = timed.then(Instant::now);
        out.clear();
        let Some(result) = catch_now(timed, || A::handle_now(route, cx, out)) else {
            return Some(Job::Decide(route));
        };
        let (failure, page) = settle::<A>(cx, out, reply, result);
        if let Some(page) = page {
            return Some(Job::Page(Failed {
                route,
                page,
                failure,
                started,
            }));
        }
        answered(cx, reply, started, failure);
    }
    tag::<A>(cx, reply);
    None
}

/// Decides the request in `b`, routed to `route`, on this thread's
/// decider: its buffers when that is done at once, else the decider, still
/// deciding it (see [`decided`]).
#[cfg(target_os = "linux")]
pub(super) fn at_once<A: App>(b: Box<Buffers>, job: Job) -> Result<Box<Buffers>, Deciding> {
    let mut d = DECIDER.take().unwrap_or_else(|| Box::pin(decider::<A>()));
    INBOX.set(Some((b, job)));
    // Taken out meanwhile: one that panics is dropped, not polled again.
    let _ = d
        .as_mut()
        .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()));
    match OUTBOX.take() {
        Some(b) => {
            DECIDER.set(Some(d));
            Ok(b)
        }
        None => Err(d),
    }
}

/// The buffers of the request the decider `d` decides, once it has.
#[cfg(not(target_arch = "wasm32"))]
pub(super) async fn decided(mut d: Deciding) -> Box<Buffers> {
    std::future::poll_fn(|cx| {
        let _ = d.as_mut().poll(cx);
        OUTBOX.take().map_or(Poll::Pending, Poll::Ready)
    })
    .await
}

/// Answers one request in process, with no socket: the same parser, limits,
/// routing, hooks and error pages as the built-in server. Run
/// [`crate::prepare`] once first, so `init` has run.
pub async fn handle<A: App>(req: Request) -> Reply {
    handle_keeping::<A>(req, &mut None).await
}

/// [`handle`], leaving in `upgrade` the WebSocket handler that `handle`
/// answers 501 for, for the test client.
pub(crate) async fn handle_keeping<A: App>(
    req: Request,
    upgrade: &mut Option<crate::ws::Upgrade>,
) -> Reply {
    let headers = req.headers.iter().map(|(n, v)| (n.as_str(), v.as_bytes()));
    match Cx::from_request::<A>(&req.method, &req.target, headers, &req.body, req.peer) {
        Ok(cx) => answer::<A>(cx, upgrade).await,
        Err(status) => Reply::plain(status),
    }
}

/// [`decide`] for a request of its own, with the page rendered and the
/// framing as [`serialize`] has it on the wire.
/// A WebSocket handler is left in `upgrade`, as [`handle_keeping`] says.
pub(crate) async fn answer<A: App>(mut cx: Cx, upgrade: &mut Option<crate::ws::Upgrade>) -> Reply {
    setup::<A>();
    let (mut out, mut reply) = (Out::default(), Reply::default());
    decide::<A>(&mut cx, &mut out, &mut reply, None).await;
    match reply.body {
        Body::WebSocket(_) => {
            if let Body::WebSocket(kept) = std::mem::replace(&mut reply.body, Body::Static(b"")) {
                *upgrade = Some(kept);
            }
            reply.set_plain(501, "WebSockets need Wisp's own server");
        }
        Body::Page => reply.body = Body::Bytes(page::<A>(&mut out).concat().into_bytes()),
        Body::Made(_) => crate::bake::unpack(&mut reply),
        _ => {}
    }
    let head = cx.method == Method::Head;
    let bodiless = bodiless(reply.status);
    // A header that would split the response is left out, as on the wire.
    reply
        .headers
        .retain(|(n, v)| valid_header(n, v) && !framing(n, head));
    // HEAD gets the headers of a GET and no body; 204, 205 and 304 have
    // none, and a 205 says so.
    if head || bodiless {
        let len = (!matches!(reply.body, Body::Stream(_))).then(|| reply.bytes().len());
        let len = if reply.status == 205 { Some(0) } else { len };
        reply.body = Body::Static(b"");
        if let Some(len) = len.filter(|_| {
            reply.header("content-length").is_none() && (!bodiless || reply.status == 205)
        }) {
            reply
                .headers
                .push((Cow::Borrowed("content-length"), Cow::Owned(len.to_string())));
        }
    }
    if out.obs.is_some() {
        crate::obs::tag(&out.obs, &mut reply);
        crate::obs::finish(&mut out.obs, reply.status, reply.bytes().len());
    }
    reply
}

impl Cx {
    /// A request from a host other than the built-in server, through the
    /// same parser and limits (header size and count, body limit, framing).
    /// `content-length` and `transfer-encoding` are the host's business and
    /// are replaced by the length of `body`, which is already whole.
    /// `Err` is the status to refuse the request with.
    pub fn from_request<'a, A: App>(
        method: &str,
        target: &str,
        headers: impl IntoIterator<Item = (&'a str, &'a [u8])>,
        body: &[u8],
        peer: SocketAddr,
    ) -> Result<Cx, u16> {
        // A control byte (but a value's tab) would end a line or a field
        // here, and let one request pass for two.
        let bad = |s: &[u8]| s.iter().any(|&b| swar::is_control(b) && b != b'\t');
        if method.is_empty()
            || !method.bytes().all(|b| b.is_ascii_alphabetic())
            || !swar::none(target.as_bytes(), |x| swar::control(x) | swar::eq(x, b' '))
        {
            return Err(400);
        }
        let mut cx = Cx::new(peer);
        let buf = &mut cx.wire.buf;
        buf.clear();
        for part in [method, " ", target, " HTTP/1.1\r\n"] {
            buf.extend_from_slice(part.as_bytes());
        }
        for (name, value) in headers {
            if name.eq_ignore_ascii_case("content-length")
                || name.eq_ignore_ascii_case("transfer-encoding")
            {
                continue;
            }
            if name.is_empty() || name.contains(':') || bad(name.as_bytes()) || bad(value) {
                return Err(400);
            }
            for part in [name.as_bytes(), b": ", value, b"\r\n"] {
                buf.extend_from_slice(part);
            }
        }
        buf.extend_from_slice(b"content-length: ");
        push_decimal(buf, body.len() as u64);
        buf.extend_from_slice(b"\r\n\r\n");
        buf.extend_from_slice(body);
        match parse::<A>(&mut cx, 0, false) {
            Parsed::Request(r) if r.len == cx.wire.buf.len() => Ok(cx),
            Parsed::Invalid(status) => Err(status),
            _ => Err(400),
        }
    }
}

/// Decides the response to the request in `cx`: Wisp's own files, the
/// app's, routing, hooks, redirects, error pages. Writes nothing; see
/// [`serialize`]. Gives it `x-request-id` when the request has an id
/// (`WISP_REQUEST_ID=on`, or `cx.request_id()`). `routed`: its route, when
/// [`route`] found it already.
///
/// What it holds across its awaits is little, the handler's future and
/// the error page's: the rest is done in plain functions around them, so
/// the future moves cheaply as it is made, every request.
pub(super) async fn decide<A: App>(
    cx: &mut Cx,
    out: &mut Out,
    reply: &mut Reply,
    routed: Option<Option<usize>>,
) {
    if crate::settings().request_id {
        cx.request_id();
    }
    // Routed first (it only matches), so the path is read once.
    let route = routed.unwrap_or_else(|| route::<A>(cx));
    if crate::obs::on() {
        crate::obs::begin(cx, route, &mut out.obs);
    }
    if !before_routes::<A>(cx, route, reply) {
        let started = crate::settings().timed.then(Instant::now);
        out.clear();
        if out.obs.is_some() {
            crate::obs::hand(&out.obs);
        }
        let result = catch_made(|| A::handle(route, cx, out)).await;
        let (failure, page) = settle::<A>(cx, out, reply, result);
        if let Some(page) = page {
            render_error::<A>(route, cx, out, page).await;
        }
        answered(cx, reply, started, failure);
    }
    tag::<A>(cx, reply);
}

/// Writes `res`, a handler's response, into `reply`.
#[inline(always)]
pub(super) fn put(cx: &Cx, reply: &mut Reply, mut res: crate::Response) {
    if not_modified(cx, &res) {
        res.status = 304;
    }
    reply.status = res.status;
    reply.headers.clear();
    // A page gets its security headers here, unless the app set them.
    if res.page {
        for (name, value) in crate::headers::missing(cx, &res.headers) {
            reply
                .headers
                .push((Cow::Borrowed(name), Cow::Borrowed(value)));
        }
    }
    if !res.content_type.is_empty() {
        reply
            .headers
            .push((Cow::Borrowed("content-type"), res.content_type));
    }
    if !res.headers.is_empty() {
        reply.add(res.headers);
    }
    reply.body = match (res.upgrade.take(), res.stream.take()) {
        (Some(upgrade), _) => Body::WebSocket(upgrade),
        (None, Some(body)) => Body::Stream(body),
        (None, None) => Body::Bytes(res.body),
    };
}

/// The reply to what the handler did, `result` ([`answer_of`]), or to its
/// error ([`error_reply`]): what went wrong, for the log, and the error
/// page to render, if any.
#[allow(clippy::type_complexity)]
#[inline(always)]
pub(super) fn settle<A: App>(
    cx: &mut Cx,
    out: &mut Out,
    reply: &mut Reply,
    result: crate::Result<()>,
) -> (Option<String>, Option<(u16, Cow<'static, str>)>) {
    // A const: without `report` in `hooks.rs`, exactly the plain call.
    if A::REPORT
        && let Err(e) = &result
        && e.status >= 500
    {
        A::report(cx, e);
    }
    settle_plain::<A>(cx, out, reply, result)
}

/// Out of line, as the caller's frame wants it: the endpoint check is on the
/// error path only, so a successful request runs no more than `answer_of`.
#[allow(clippy::type_complexity)]
#[inline(never)]
pub(super) fn settle_plain<A: App>(
    cx: &mut Cx,
    out: &mut Out,
    reply: &mut Reply,
    result: crate::Result<()>,
) -> (Option<String>, Option<(u16, Cow<'static, str>)>) {
    match answer_of(cx, out, reply, result) {
        Ok(()) => (None, None),
        Err(e) => {
            // Consts: an app with pages and no endpoint-only prefix has no
            // check.
            if A::API_ONLY || !A::API_PREFIXES.is_empty() {
                endpoint_error::<A>(cx);
            }
            error_reply(cx, out, reply, e)
        }
    }
}

/// [`tag_plain`], then `after` when the app has one: the last thing every
/// answer gets.
#[inline(always)]
pub(super) fn tag<A: App>(cx: &mut Cx, reply: &mut Reply) {
    tag_plain(cx, reply);
    // A const: no code at all without `headers` in the app's config.
    if A::HEADERS {
        A::headers(cx, reply);
    }
    // A const: no code at all unless `hooks.rs` has `after`.
    if A::AFTER {
        A::after(cx, reply);
    }
}

/// The reply's `x-request-id`, when the request has an id, and its HSTS
/// (`WISP_HSTS=on`).
pub(super) fn tag_plain(cx: &Cx, reply: &mut Reply) {
    if let Some(id) = cx.id() {
        reply
            .headers
            .push((Cow::Borrowed("x-request-id"), Cow::Owned(id.to_string())));
    }
    if crate::headers::HSTS_ON.load(std::sync::atomic::Ordering::Relaxed) {
        crate::headers::hsts(reply);
    }
}

/// What is answered before the routes: a path that is not one, Wisp's own
/// files, a trailing slash, the app's files. Whether it was.
pub(super) fn before_routes<A: App>(cx: &Cx, route: Option<usize>, reply: &mut Reply) -> bool {
    // Its bytes: only a few of these need it as a `str`.
    let raw = cx.raw_path();
    if raw.first() != Some(&b'/') {
        reply.set_plain(400, "Bad Request");
        return true;
    }
    if raw.starts_with(b"/_") && internal::<A>(cx, cx.path(), reply) {
        return true;
    }
    // A const: no code at all without `redirects` in the app's config.
    if A::REDIRECTS && A::redirect(cx, reply) {
        return true;
    }
    // No route: the fallbacks, out of the way of the ones that have one.
    let Some(r) = route else {
        return unrouted::<A>(cx, raw, reply);
    };
    if raw.len() > 1 {
        if raw.ends_with(b"/") {
            if slash() == TrailingSlash::Never {
                slash_redirect(cx, reply, false);
                return true;
            }
        } else if A::TRAILING_SLASH
            && A::ROUTES[r].page
            && matches!(cx.method, Method::Get | Method::Head)
            && slash() == TrailingSlash::Always
            && !cx
                .path()
                .rsplit('/')
                .next()
                .is_some_and(|last| last.contains('.'))
        {
            slash_redirect(cx, reply, true);
            return true;
        }
    }
    matches!(cx.method, Method::Get | Method::Head) && file::<A>(cx, raw, route, reply)
}

/// [`before_routes`] of a path no route matches: a trailing slash is
/// redirected from, then the app's files, and `/sitemap.xml` and
/// `/robots.txt` when no file is there. Cold: only a miss gets here.
#[cold]
#[inline(never)]
pub(super) fn unrouted<A: App>(cx: &Cx, raw: &[u8], reply: &mut Reply) -> bool {
    if raw.len() > 1 && raw.ends_with(b"/") {
        slash_redirect(cx, reply, false);
        return true;
    }
    if !matches!(cx.method, Method::Get | Method::Head) {
        return false;
    }
    if file::<A>(cx, raw, None, reply) {
        return true;
    }
    let Some((body, mime)) = crate::seo::answer::<A>(cx) else {
        return false;
    };
    reply.set(200, mime, Body::Bytes(body));
    true
}

/// The reply to what the handler did, `result`: its response, page or
/// redirect; or the error an error page answers.
pub(super) fn answer_of(
    cx: &mut Cx,
    out: &mut Out,
    reply: &mut Reply,
    mut result: crate::Result<()>,
) -> Result<(), Error> {
    if result.is_ok()
        && let Some(res) = out.response.as_mut().filter(|r| r.upgrade.is_some())
    {
        result = crate::ws::handshake(cx).map(|accept| {
            res.headers
                .push((Cow::Borrowed("sec-websocket-accept"), accept));
        });
    }
    match result {
        Ok(()) => {
            match out.response.take() {
                Some(res) => put(cx, reply, res),
                None => match out.made.take() {
                    Some(made) => {
                        // A kept page was made with them.
                        if matches!(made, crate::bake::Made::Baked(_)) {
                            crate::headers::page(cx);
                        }
                        crate::bake::reply(cx, made, reply);
                    }
                    None => {
                        crate::headers::page(cx);
                        reply.set(cx.status(), "text/html; charset=utf-8", Body::Page);
                    }
                },
            }
            Ok(())
        }
        Err(e) if e.status < 400 => {
            redirect_reply(cx, e, reply);
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// The reply decided: given the headers the
/// request set, and logged; `failure` is what went wrong in a 5xx, for the
/// log (the page may say less).
pub(super) fn answered(
    cx: &mut Cx,
    reply: &mut Reply,
    started: Option<Instant>,
    mut failure: Option<String>,
) {
    if cx.unsigned() {
        *reply = Reply::default();
        reply.set_plain(500, "Internal Server Error");
        failure = Some(crate::sign::NO_SECRET.into());
        cx.drop_page_headers();
    }
    cx.send_headers(&mut reply.headers);
    let method = cx.method.as_str();
    if let Some(started) = started
        && crate::settings().server_timing
    {
        server_timing(cx, reply, started);
    }
    if let Some(started) = started.filter(|_| crate::settings().dev) {
        let blocked = Some(BLOCKED.replace(Duration::ZERO)).filter(|&b| b >= BLOCKING);
        let (path, id) = (cx.path(), cx.id());
        dev::log_request(
            method,
            path,
            reply.status,
            started.elapsed(),
            failure.as_deref(),
            blocked,
            id,
        );
    } else if let Some(f) = failure {
        let id = cx.id().map_or(String::new(), |id| format!(" [{id}]"));
        log(format_args!(
            "wisp: {} {method} {}{id}: {f}",
            reply.status,
            cx.path()
        ));
    }
}

/// Marks phase `i` of the request (0: the `before` hook done, 1: the page
/// renders) for [`server_timing`]. Dev builds only.
#[cfg(debug_assertions)]
pub(crate) fn mark(cx: &mut Cx, i: usize) {
    if crate::settings().server_timing {
        cx.marks[i] = Some(Instant::now());
    }
}

/// `WISP_SERVER_TIMING`: `Server-Timing: total;dur=1.20` on the answer; a
/// dev build splits it into `before` (the hook), `handler` (loads, actions,
/// endpoints) and `render`, each that ran.
pub(super) fn server_timing(cx: &Cx, reply: &mut Reply, started: Instant) {
    #[cfg(debug_assertions)]
    let marks = cx.marks;
    #[cfg(not(debug_assertions))]
    let marks = [None; 2];
    let _ = cx;
    let value = timing(started, marks, Instant::now());
    reply.headers.push(("server-timing".into(), value.into()));
}

/// The `Server-Timing` value of a request that began at `started` and
/// ended at `end`, with the phase marks it made. A mark from before
/// `started` (another request's) is not this one's.
pub(super) fn timing(started: Instant, marks: [Option<Instant>; 2], end: Instant) -> String {
    let ms = |from: Instant, to: Instant| to.saturating_duration_since(from).as_secs_f64() * 1000.0;
    let [hooked, render] = marks.map(|m| m.filter(|&m| m >= started && m <= end));
    let mut s = format!("total;dur={:.2}", ms(started, end));
    if let Some(h) = hooked {
        s += &format!(", before;dur={:.2}", ms(started, h));
    }
    if hooked.is_some() || render.is_some() {
        let from = hooked.unwrap_or(started);
        s += &format!(", handler;dur={:.2}", ms(from, render.unwrap_or(end)));
    }
    if let Some(r) = render {
        s += &format!(", render;dur={:.2}", ms(r, end));
    }
    s
}

/// Whether `res`, a 200 to a GET or HEAD with an `etag` (an [`crate::Image`],
/// say), is what the client says it has: then it is a 304, with no body.
/// The response's own headers are few; the request's are looked through
/// only for one with an `etag`.
pub(super) fn not_modified(cx: &Cx, res: &crate::Response) -> bool {
    res.status == 200
        && matches!(cx.method, Method::Get | Method::Head)
        && (res.headers.iter()).any(|(n, v)| n == "etag" && fresh(cx, v))
}

/// Whether the request's `if-none-match` names `etag`, weakly as a GET
/// compares (`W/"x"` is `"x"`), or is `*`: the client has it, a 304.
pub(crate) fn fresh(cx: &Cx, etag: &str) -> bool {
    cx.known(Known::IfNoneMatch)
        .is_some_and(|h| crate::rest::names::<true>(h, etag))
}

/// A redirect. Headers set before it (a login cookie) still apply.
/// wisp.js gets it as `x-wisp-location` and goes there itself: fetch would
/// follow it with the post's own headers, and to another site (a payment
/// page) not at all.
pub(super) fn redirect_reply(cx: &Cx, e: Error, reply: &mut Reply) {
    let js = cx.header(crate::protocol::HEADER_JS).is_some();
    reply.set_plain(if js { 200 } else { e.status }, "");
    if let Some((name, value)) = e.header.map(|h| *h) {
        let name = if js && name == "location" {
            crate::protocol::HEADER_LOCATION
        } else {
            name
        };
        crate::headers::set_in(&mut reply.headers, name, value);
    }
}

/// The error page of `status` with `message`: the nearest `+error.wisp`,
/// else Wisp's own.
pub(super) async fn render_error<A: App>(
    route: Option<usize>,
    cx: &mut Cx,
    out: &mut Out,
    (status, message): (u16, Cow<'static, str>),
) {
    let rendered = catch_made(|| A::error(route, cx, out, status, &message)).await;
    if rendered.is_err() || out.response.is_some() {
        out.clear();
        rt::default_error(cx, out, status, &message);
    }
}

/// The reply to a 4xx or 5xx: JSON for a client that wants that, else an
/// error page, whose status and message come back for [`render_error`].
/// And what went wrong in a 5xx, for the log.
#[allow(clippy::type_complexity)]
pub(super) fn error_reply(
    cx: &mut Cx,
    out: &mut Out,
    reply: &mut Reply,
    mut e: Error,
) -> (Option<String>, Option<(u16, Cow<'static, str>)>) {
    let failure = (e.status >= 500).then(|| e.detail());
    // 5xx details can leak internals; only dev shows them. An error that
    // says no more than its status's name has no message of its own.
    let own = (e.status < 500 || crate::settings().dev)
        && !e.message.is_empty()
        && !e.message.eq_ignore_ascii_case(reason(e.status));
    out.clear();
    // The headers of the page that failed go with it; the `before` hook's
    // stay.
    cx.drop_page_headers();
    crate::headers::page(cx);
    let page = if error_json(cx) {
        // The message, else the status's name.
        let message = match own {
            true => std::mem::take(&mut e.message),
            false => Cow::Borrowed(title(e.status)),
        };
        let problem = crate::settings().problem_json
            || cx
                .known(Known::Accept)
                .is_some_and(|a| a.contains("application/problem+json"));
        let body = e.json(&message, problem).into_bytes();
        let kind = match problem {
            true => "application/problem+json",
            false => "application/json",
        };
        reply.set(e.status, kind, Body::Bytes(body));
        None
    } else {
        // A page says a sentence on the status instead; in dev, with what
        // caused the error.
        let message = match (own, crate::settings().dev) {
            (false, _) => Cow::Borrowed(sentence(e.status)),
            (true, true) => Cow::Owned(e.detail()),
            (true, false) => std::mem::take(&mut e.message),
        };
        reply.set(e.status, "text/html; charset=utf-8", Body::Page);
        Some((e.status, message))
    };
    if let Some((name, value)) = e.header.take().map(|h| *h) {
        crate::headers::set_in(&mut reply.headers, name, value);
    }
    (failure, page)
}

/// Whether a client wants JSON rather than a page, to be told it must sign
/// in: a request under `/api`, one that sent JSON, one that asks for JSON
/// and not HTML, or one to a `+server.rs` endpoint from anything but a
/// browser page.
pub(crate) fn wants_json(cx: &Cx) -> bool {
    let path = cx.path();
    path == "/api"
        || path.starts_with("/api/")
        || crate::input::is_json(cx)
        || crate::input::asks_json(cx)
        || (cx.api()
            && !cx
                .known(Known::Accept)
                .is_some_and(|a| a.contains("text/html")))
}

/// Whether an error goes back as JSON rather than an error page: whatever
/// [`wants_json`] says, any request to an endpoint (a `+server.rs` route,
/// or a path where only those are, or in an app of nothing else), one that
/// prefers JSON to HTML by its `Accept`, or one with none that is not a
/// browser navigating.
pub(super) fn error_json(cx: &Cx) -> bool {
    wants_json(cx)
        || cx.api()
        || match cx.known(Known::Accept) {
            Some(accept) => prefers_json(accept),
            None => cx.header("sec-fetch-mode") != Some("navigate"),
        }
}

/// Whether `accept` names JSON before it names HTML, or names JSON alone.
pub(super) fn prefers_json(accept: &str) -> bool {
    match (accept.find("json"), accept.find("text/html")) {
        (Some(j), Some(h)) => j < h,
        (j, _) => j.is_some(),
    }
}

/// An error where only endpoints are, or in an app of nothing else, is an
/// endpoint's: JSON. The routes were found already; only an unmatched
/// path looks at its first segment.
#[cold]
#[inline(never)]
pub(super) fn endpoint_error<A: App>(cx: &mut Cx) {
    if A::API_ONLY {
        return cx.set_api();
    }
    let path = cx.path();
    let first = path.strip_prefix('/').unwrap_or(path);
    let first = first.split('/').next().unwrap_or("");
    if A::route(path).is_none() && A::API_PREFIXES.contains(&first) {
        cx.set_api();
    }
}
