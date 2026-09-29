//! HTTP/1.1 server.
//!
//! One task per connection. Each connection owns a `Cx` (which owns the read
//! buffer), a write buffer and an `Out`, all reused across requests, so a
//! warm connection allocates nothing for a typical page. Requests are parsed
//! in place and every complete request in the read buffer is answered
//! before a single write, which gives pipelining for free.
//!
//! Deliberately not here: TLS, HTTP/2, compression. A reverse proxy or CDN
//! does those better.
//!
//! The edge build (wasm32) has no sockets: it keeps `handle` and what it
//! needs, and leaves the server out.
#![cfg_attr(target_arch = "wasm32", allow(dead_code, unused_imports))]

use crate::cx::{Cx, Method, Span, decode, valid_header};
use crate::idem::Start;
use crate::{App, Error, Out, dev, rt};
use std::borrow::Cow;
use std::cell::Cell;
use std::future::Future;
use std::io::{self, Write};
use std::mem::MaybeUninit;
use std::net::SocketAddr;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::task::Poll;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
#[cfg(not(target_arch = "wasm32"))]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(not(target_arch = "wasm32"))]
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, mpsc};

const MAX_HEADERS: usize = 64;
const MAX_HEAD: usize = 16 * 1024;
const MAX_SEGS: usize = 32;
/// Time allowed to receive a request's head once its first byte arrived,
/// and then for each part of its body: a large upload may take minutes, as
/// long as it keeps coming.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Bytes a second a request body must average, after `REQUEST_TIMEOUT`.
const MIN_BODY_RATE: usize = 1024;
/// Time an idle keep-alive connection is kept open.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// Time a client may take none of a response before it is dropped.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// Time a stopping server waits for the requests under way.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);
/// A handler that holds its thread longer than this in one go is reported
/// in dev: every other connection on that thread waited meanwhile.
const BLOCKING: Duration = Duration::from_millis(100);
/// A buffer that grew past this for one large message is shrunk afterwards,
/// so memory per idle connection stays bounded.
const KEEP_CAPACITY: usize = 64 * 1024;

/// The browser runtime: as written in dev builds, without comments and
/// indentation in release ones (see `build.rs`). The ETag tells them apart.
#[cfg(debug_assertions)]
const CLIENT_JS: &[u8] = include_bytes!("client/wisp.js");
#[cfg(not(debug_assertions))]
const CLIENT_JS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/wisp.js"));
#[cfg(debug_assertions)]
const CLIENT_JS_ETAG: &str = concat!("\"", env!("CARGO_PKG_VERSION"), "-dev\"");
#[cfg(not(debug_assertions))]
const CLIENT_JS_ETAG: &str = concat!("\"", env!("CARGO_PKG_VERSION"), "\"");
/// The runtime of client scripts and directives, linked by pages that
/// render any (see `live.rs`). Versioned like `wisp.js`.
#[cfg(debug_assertions)]
const LIVE_JS: &[u8] = include_bytes!("client/live.js");
#[cfg(not(debug_assertions))]
const LIVE_JS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/live.js"));
/// Live reload and the build error dialog, with the dialog's styles. Served
/// and linked only by debug builds, so none of it ships in a release
/// binary's pages.
const DEV_JS: &[u8] = include_bytes!("client/wisp-dev.js");
/// Also inlined into the fallback error page (`rt::default_error`).
pub(crate) const UI_CSS: &str = include_str!("client/ui.css");
const DIALOG_CSS: &[u8] = include_bytes!("client/dialog.css");

/// The page at `/_wisp/docs` that lists the app's endpoints and sends
/// requests to them, with Wisp's own styles.
fn api_docs() -> &'static [u8] {
    static PAGE: OnceLock<String> = OnceLock::new();
    PAGE.get_or_init(|| include_str!("api-docs.html").replace("/*ui.css*/", UI_CSS))
        .as_bytes()
}

/// `<link>`/`<script>` tags for `%wisp.head%`. Fixed for the process.
static HEAD_TAGS: OnceLock<String> = OnceLock::new();

/// Thread per core: `threads` workers, each a single-threaded tokio runtime
/// with its own I/O driver and timers, and this thread accepting connections
/// and handing each to the worker with the fewest. A connection stays on one
/// thread for its whole life, so the request path never wakes another thread
/// or shares a driver. (A multi-threaded tokio runtime funnels every socket event
/// through one driver; it measured at under half the throughput with cores
/// left idle.)
///
/// Returns on SIGTERM or Ctrl+C, once it has stopped accepting and the
/// requests under way have been answered (see [`stop`]).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn run<A: App>(addr: SocketAddr, threads: usize) -> io::Result<()> {
    // `init` runs on this thread's runtime, which serves until the process
    // ends, so what it connects (a database pool) keeps being driven.
    let main = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    main.block_on(crate::prepare::<A>())?;

    let listener = bind(addr)?;
    listener.set_nonblocking(true)?;
    let workers = (0..threads.max(1))
        .map(|i| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let handle = runtime.handle().clone();
            std::thread::Builder::new()
                .name(format!("wisp-{i}"))
                .spawn(move || runtime.block_on(std::future::pending::<()>()))?;
            Ok(handle)
        })
        .collect::<io::Result<Vec<_>>>()?;

    main.block_on(async {
        let listener = TcpListener::from_std(listener)?;
        started(listener.local_addr()?);
        let mut signal = std::pin::pin!(stop_signal());
        let max = crate::settings().max_conns;
        let open: std::sync::Arc<[AtomicUsize]> =
            workers.iter().map(|_| AtomicUsize::new(0)).collect();
        let mut next = 0;
        while let Some(accepted) = first(async { Some(listener.accept().await) }, async {
            signal.as_mut().await;
            None
        })
        .await
        {
            match accepted {
                Ok((stream, peer)) => {
                    let Some(slot) = Slot::take(max) else {
                        refuse(stream);
                        continue;
                    };
                    let _ = stream.set_nodelay(true);
                    // Moved to the worker's driver, which serves it from here on.
                    let Ok(stream) = stream.into_std() else {
                        continue;
                    };
                    // The worker with the fewest open connections, looking
                    // from `next` so that ties take turns: plain turns drift
                    // apart as connections close unevenly, leaving one core
                    // idle while another queues.
                    let w = (0..workers.len())
                        .map(|k| (next + k) % workers.len())
                        .min_by_key(|&i| open[i].load(Ordering::Relaxed))
                        .unwrap_or(0);
                    next = (w + 1) % workers.len();
                    open[w].fetch_add(1, Ordering::Relaxed);
                    let held = Held(open.clone(), w);
                    workers[w].spawn(async move {
                        if let Ok(stream) = TcpStream::from_std(stream) {
                            connection::<A>(stream, peer).await;
                        }
                        drop((slot, held));
                    });
                }
                Err(e) => {
                    if accept_failed(&e) {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
        }
        drop(listener);
        stop(&workers).await;
        Ok(())
    })
}

/// Stopping: new connections are already refused. A connection answers
/// what it is receiving or working on with `connection: close` and closes;
/// an idle one is closed as the process exits, as nginx does, and a client
/// retries on another connection. Waits at most `DRAIN_TIMEOUT`, or until a
/// second signal.
#[cfg(not(target_arch = "wasm32"))]
async fn stop(workers: &[tokio::runtime::Handle]) {
    STOPPING.store(true, Ordering::Relaxed);
    STOP.notify_waiters(); // ends streamed responses
    log(format_args!("wisp: stopping"));
    let drained = async {
        let until = Instant::now() + DRAIN_TIMEOUT;
        while Instant::now() < until {
            let (tx, rx) = std::sync::mpsc::channel();
            for w in workers {
                let tx = tx.clone();
                w.spawn(async move {
                    let _ = tx.send(BUSY.get());
                });
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
            let busy: Vec<isize> = rx.try_iter().collect();
            if busy.len() == workers.len() && busy.iter().sum::<isize>() == 0 {
                return;
            }
        }
        log(format_args!(
            "wisp: requests still under way after {} s; stopping anyway",
            DRAIN_TIMEOUT.as_secs()
        ));
    };
    first(drained, stop_signal()).await;
}

/// SIGTERM, which is how systemd, Docker and Kubernetes stop a server, or
/// Ctrl+C (SIGINT).
#[cfg(not(target_arch = "wasm32"))]
async fn stop_signal() {
    async fn ctrl_c() {
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut term) = signal(SignalKind::terminate()) {
            let term = async {
                term.recv().await;
            };
            return first(term, ctrl_c()).await;
        }
    }
    ctrl_c().await
}

/// Runs two futures at once and returns the output of the first to finish.
pub(crate) async fn first<T>(a: impl Future<Output = T>, b: impl Future<Output = T>) -> T {
    let (mut a, mut b) = (std::pin::pin!(a), std::pin::pin!(b));
    std::future::poll_fn(move |cx| match a.as_mut().poll(cx) {
        Poll::Ready(v) => Poll::Ready(v),
        Poll::Pending => b.as_mut().poll(cx),
    })
    .await
}

/// Set once the server is stopping.
static STOPPING: AtomicBool = AtomicBool::new(false);
/// Wakes what waits for the server to stop.
static STOP: Notify = Notify::const_new();

/// Whether the server is stopping.
pub(crate) fn stopping() -> bool {
    STOPPING.load(Ordering::Relaxed)
}

/// Resolves once the server is stopping.
pub(crate) async fn stopped() {
    let wait = STOP.notified();
    let mut wait = std::pin::pin!(wait);
    wait.as_mut().enable(); // before the check, so no notification is missed
    if !STOPPING.load(Ordering::Relaxed) {
        wait.await;
    }
}

thread_local! {
    /// Connections on this thread with a request under way. Summed over the
    /// workers when stopping. A task that moved between threads (under
    /// [`serve`]) can leave one thread's count negative; the sum stays right.
    static BUSY: Cell<isize> = const { Cell::new(0) };
}

/// Counts its connection in `BUSY` while set.
struct Busy(bool);

impl Busy {
    fn set(&mut self, busy: bool) {
        if busy != self.0 {
            BUSY.set(BUSY.get() + if busy { 1 } else { -1 });
            self.0 = busy;
        }
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.set(false);
    }
}

/// Connections open now, WebSockets included, against `WISP_MAX_CONNS`.
#[cfg(not(target_arch = "wasm32"))]
static CONNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// One of `CONNS`, held for its connection's life.
#[cfg(not(target_arch = "wasm32"))]
struct Slot;

#[cfg(not(target_arch = "wasm32"))]
impl Slot {
    /// A slot, or `None` at the cap. Counted before the check, so two at
    /// once cannot both take the last one; a refused one uncounts as it drops.
    fn take(max: usize) -> Option<Slot> {
        let slot = Slot;
        (CONNS.fetch_add(1, Ordering::Relaxed) < max).then_some(slot)
    }
}

/// A connection over the cap: told 503 (its send buffer, new and empty,
/// takes it without waiting) and closed, before it costs a task or a read.
#[cfg(not(target_arch = "wasm32"))]
fn refuse(stream: TcpStream) {
    if let Ok(s) = stream.into_std() {
        let _ = (&s).write_all(b"HTTP/1.1 503 Service Unavailable\r\nretry-after: 1\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Slot {
    fn drop(&mut self) {
        CONNS.fetch_sub(1, Ordering::Relaxed);
    }
}

/// A connection counted against its worker in [`run`], until it drops.
#[cfg(not(target_arch = "wasm32"))]
struct Held(std::sync::Arc<[AtomicUsize]>, usize);

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Held {
    fn drop(&mut self) {
        self.0[self.1].fetch_sub(1, Ordering::Relaxed);
    }
}

/// Binds, with what to do about the usual failures.
fn bind(addr: SocketAddr) -> io::Result<std::net::TcpListener> {
    std::net::TcpListener::bind(addr).map_err(|e| {
        let hint = match e.kind() {
            io::ErrorKind::AddrInUse => "Another program, maybe another copy of this one, is using the port. Stop it, or set PORT to use another.",
            io::ErrorKind::PermissionDenied => "The port is reserved or needs more privileges (below 1024 on Linux). Set PORT to use another.",
            io::ErrorKind::AddrNotAvailable => "This machine has no such address. Set HOST to one of its own, or to 0.0.0.0 for all.",
            _ => "",
        };
        io::Error::new(e.kind(), format!("cannot listen on {addr}: {e}\n  {hint}").trim_end().to_string())
    })
}

/// Reads more into `buf`: `Ok(0)` when the peer closed, `TimedOut` if
/// nothing came by `deadline` (in `seconds()`).
///
/// `timer` is the connection's one timer. Deadlines are whole seconds, so
/// it is moved at most once a second, and a later deadline only updates it
/// in place: a timer made and dropped per read cost two lock round trips on
/// the timer wheel and a clock read, every request.
#[cfg(not(target_arch = "wasm32"))]
async fn read(
    stream: &mut TcpStream,
    buf: &mut Vec<u8>,
    mut timer: std::pin::Pin<&mut tokio::time::Sleep>,
    deadline: u64,
) -> io::Result<usize> {
    let when = instant(deadline);
    if timer.deadline() != when {
        timer.as_mut().reset(when);
    }
    first(stream.read_buf(buf), async {
        timer.await;
        Err(io::ErrorKind::TimedOut.into())
    })
    .await
}

/// Runs `f`, giving up with `TimedOut` after `limit`. A timer is only set
/// up when `f` cannot finish at once, which most writes can.
#[cfg(not(target_arch = "wasm32"))]
async fn within<T>(limit: Duration, f: impl Future<Output = io::Result<T>>) -> io::Result<T> {
    let mut f = std::pin::pin!(f);
    if let Poll::Ready(done) = std::future::poll_fn(|cx| Poll::Ready(f.as_mut().poll(cx))).await {
        return done;
    }
    tokio::time::timeout(limit, f)
        .await
        .unwrap_or_else(|_| Err(io::ErrorKind::TimedOut.into()))
}

/// Writes all of `buf`, giving up on a client that has taken none of it for
/// `WRITE_TIMEOUT`, so one that stops reading cannot hold a connection (and
/// its buffers) forever. A slow client that keeps reading is fine.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn write(stream: &mut (impl AsyncWriteExt + Unpin), buf: &[u8]) -> io::Result<()> {
    let mut at = 0;
    while at < buf.len() {
        match within(WRITE_TIMEOUT, stream.write(&buf[at..])).await? {
            0 => return Err(io::ErrorKind::WriteZero.into()),
            n => at += n,
        }
    }
    Ok(())
}

/// Serves from inside a runtime the caller owns, until the future is
/// dropped. Stopping gracefully is up to the caller.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn serve<A: App>(addr: SocketAddr) -> io::Result<()> {
    crate::prepare::<A>().await?;
    let listener = TcpListener::from_std({
        let l = bind(addr)?;
        l.set_nonblocking(true)?;
        l
    })?;
    started(listener.local_addr()?);
    let max = crate::settings().max_conns;
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let Some(slot) = Slot::take(max) else {
                    refuse(stream);
                    continue;
                };
                let _ = stream.set_nodelay(true);
                tokio::spawn(async move {
                    connection::<A>(stream, peer).await;
                    drop(slot);
                });
            }
            Err(e) => {
                if accept_failed(&e) {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }
}

/// What the server alone needs: the clock behind `date` and timeouts, and
/// the line `wisp dev` waits for.
fn started(addr: SocketAddr) {
    start_clock();
    dev::exit_with_parent();
    // `wisp dev` waits for this exact line to know the app is ready.
    let mut stdout = io::stdout().lock();
    let _ = writeln!(stdout, "wisp: listening on http://{addr}").and_then(|()| stdout.flush());
}

/// What every host needs before the first request, whether or not it runs
/// the built-in server. Cheap to call again.
pub(crate) fn setup<A: App>() {
    install_panic_hook();
    let _ = crate::sign::ROOT.set(A::ROOT);
    if cfg!(debug_assertions) {
        dev::listed(A::ROOT, "/"); // lists `static/` now, not in the first request
    }
    HEAD_TAGS.get_or_init(|| {
        let mut s = String::new();
        if let Some(v) = A::CSS {
            s.push_str(&format!(
                "<link rel=\"stylesheet\" href=\"/_app/app.css?v={v}\">"
            ));
        }
        let version = env!("CARGO_PKG_VERSION");
        s.push_str(&format!(
            "<script defer src=\"/_app/wisp.js?v={version}\"></script>"
        ));
        if let Some(port) = dev::events_port() {
            s.push_str(&format!(
                "<script defer src=\"/_app/wisp-dev.js\" data-port=\"{port}\"></script>"
            ));
        }
        s
    });
}

/// A line on stderr. Unlike `eprintln!`, a log that cannot be written (its
/// reader gone) never takes a request down with it.
pub(crate) fn log(line: std::fmt::Arguments) {
    #[cfg(target_arch = "wasm32")]
    crate::edge::log(&line.to_string());
    #[cfg(not(target_arch = "wasm32"))]
    let _ = writeln!(io::stderr().lock(), "{line}");
}

/// A client that gave up before we accepted is routine. Anything else (out
/// of file descriptors...) is logged, and the caller should back off
/// instead of spinning: the return value says so.
fn accept_failed(e: &std::io::Error) -> bool {
    use std::io::ErrorKind::{ConnectionAborted, ConnectionReset, Interrupted};
    if matches!(e.kind(), ConnectionAborted | ConnectionReset | Interrupted) {
        return false;
    }
    log(format_args!("wisp: accept failed: {e}"));
    true
}

enum Parsed {
    /// A complete request is described by `cx`; it occupies `len` bytes.
    Request { len: usize, keep_alive: bool },
    /// More bytes are needed. `need` is the full request size when known;
    /// `body` is set once the head is in.
    Partial {
        need: usize,
        expect_continue: bool,
        body: bool,
    },
    /// Malformed or refused: answer with this status and close.
    Invalid(u16),
}

#[cfg(not(target_arch = "wasm32"))]
async fn connection<A: App>(mut stream: TcpStream, peer: SocketAddr) {
    let mut cx = Cx::new(peer);
    let mut wbuf: Vec<u8> = Vec::with_capacity(16 * 1024);
    let mut out = Out::default();
    let mut reply = Reply::default();
    let mut busy = Busy(false);
    let mut head_since: Option<u64> = None;
    let mut body_since: Option<u64> = None;
    let mut sent_continue = false;
    let mut timer = std::pin::pin!(tokio::time::sleep_until(instant(0)));

    loop {
        let mut used = 0;
        let mut need = 0;
        let mut in_body = false;
        let mut close = false;
        while used < cx.buf.len() {
            match parse::<A>(&mut cx, used) {
                Parsed::Request { len, keep_alive } => {
                    let keep_alive = keep_alive && !STOPPING.load(Ordering::Relaxed);
                    decide::<A>(&mut cx, &mut out, &mut reply).await;
                    // Only as a 101: a hook may have answered otherwise.
                    let upgrade = reply.take_websocket().filter(|_| reply.status == 101);
                    let streamed = serialize::<A>(
                        &mut wbuf,
                        &mut reply,
                        &out,
                        cx.http11,
                        keep_alive || upgrade.is_some(),
                        cx.method == Method::Head,
                    );
                    used += len;
                    // Answers to many small pipelined requests go out in
                    // pieces, so a buffer of them cannot make a huge one.
                    if streamed.is_none() && upgrade.is_none() && wbuf.len() >= KEEP_CAPACITY {
                        if write(&mut stream, &wbuf).await.is_err() {
                            return;
                        }
                        wbuf.clear();
                    }
                    if let Some(upgrade) = upgrade {
                        // The rest of the connection is the WebSocket's,
                        // with what the client sent after its handshake.
                        if write(&mut stream, &wbuf).await.is_ok() {
                            let early = cx.buf[used..].to_vec();
                            let limit = body_limit::<A>(cx.path());
                            crate::ws::serve(stream, early, limit, upgrade, cx.path()).await;
                        }
                        return;
                    }
                    head_since = None;
                    body_since = None;
                    sent_continue = false;
                    if let Some(s) = streamed {
                        // What was answered so far goes first, then the body as
                        // it comes. Bytes the client sends meanwhile (a request
                        // after this one) wait in `early`.
                        let mut early = Vec::new();
                        if write(&mut stream, &wbuf).await.is_err() {
                            return;
                        }
                        wbuf.clear();
                        if pump(&mut stream, s.body, s.chunked, &mut wbuf, &mut early)
                            .await
                            .is_err()
                        {
                            return;
                        }
                        cx.buf.extend_from_slice(&early);
                        if s.close {
                            close = true;
                            break;
                        }
                    }
                    if !keep_alive {
                        close = true;
                        break;
                    }
                }
                Parsed::Partial {
                    need: n,
                    expect_continue,
                    body,
                } => {
                    need = n;
                    in_body = body;
                    if expect_continue && !sent_continue {
                        wbuf.extend_from_slice(b"HTTP/1.1 100 Continue\r\n\r\n");
                        sent_continue = true;
                    }
                    break;
                }
                Parsed::Invalid(status) => {
                    reply.set_plain(status, reason(status));
                    serialize::<A>(&mut wbuf, &mut reply, &out, true, false, false);
                    close = true;
                    break;
                }
            }
        }

        if !wbuf.is_empty() {
            if write(&mut stream, &wbuf).await.is_err() {
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
        // Room for the rest of the body, but at most 1 MB ahead of what came:
        // a large `content-length` alone is not a reason to allocate.
        cx.buf
            .reserve(need.saturating_sub(cx.buf.len()).clamp(4096, 1024 * 1024));

        let deadline = if cx.buf.is_empty() {
            busy.set(false);
            if STOPPING.load(Ordering::Relaxed) {
                return;
            }
            seconds() + IDLE_TIMEOUT.as_secs()
        } else if in_body {
            // Each part in time, and the whole at `MIN_BODY_RATE` at least
            // after the same grace: a body sent a byte at a time cannot hold
            // a connection for days.
            let since = *body_since.get_or_insert_with(seconds);
            let rate = since + REQUEST_TIMEOUT.as_secs() + (cx.buf.len() / MIN_BODY_RATE) as u64;
            (seconds() + REQUEST_TIMEOUT.as_secs()).min(rate)
        } else {
            *head_since.get_or_insert_with(seconds) + REQUEST_TIMEOUT.as_secs()
        };
        match read(&mut stream, &mut cx.buf, timer.as_mut(), deadline).await {
            Ok(n) if n > 0 => busy.set(true),
            _ => return, // closed, error or too slow
        }
    }
}

/// Parses the request starting at `cx.buf[at..]` and, if it is complete,
/// records it in `cx` as spans.
fn parse<A: App>(cx: &mut Cx, at: usize) -> Parsed {
    cx.reset();
    let buf = &cx.buf[..];
    // Left uninitialized: zeroing 64 headers cost more than parsing a small request.
    let mut raw = [const { MaybeUninit::uninit() }; MAX_HEADERS];
    let mut req = httparse::Request::new(&mut []);
    let head_len = match req.parse_with_uninit_headers(&buf[at..], &mut raw) {
        Ok(httparse::Status::Complete(n)) if n <= MAX_HEAD => n,
        Ok(httparse::Status::Partial) if buf.len() - at <= MAX_HEAD => {
            return Parsed::Partial {
                need: 0,
                expect_continue: false,
                body: false,
            };
        }
        Ok(_) | Err(httparse::Error::TooManyHeaders) => return Parsed::Invalid(431),
        Err(_) => return Parsed::Invalid(400),
    };

    let mut content_length: Option<usize> = None;
    let mut transfer_encodings = 0;
    let mut keep_alive = req.version == Some(1);
    let mut expect_continue = false;
    cx.headers.clear();
    for h in req.headers.iter() {
        cx.headers
            .push((Span::of(buf, h.name.as_bytes()), Span::of(buf, h.value)));
        // Only these four matter here; their lengths tell most others apart
        // without comparing a byte.
        if !matches!(h.name.len(), 6 | 10 | 14 | 17) {
            continue;
        }
        if h.name.eq_ignore_ascii_case("content-length") {
            // Strict: digits only, and repeated headers must agree (smuggling).
            match (parse_decimal(h.value), content_length) {
                (Some(n), None) => content_length = Some(n),
                (Some(n), Some(m)) if n == m => {}
                _ => return Parsed::Invalid(400),
            }
        } else if h.name.eq_ignore_ascii_case("transfer-encoding") {
            // Only `chunked`, alone: gzip and the like are for a proxy.
            if !h.value.trim_ascii().eq_ignore_ascii_case(b"chunked") {
                return Parsed::Invalid(501);
            }
            transfer_encodings += 1;
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
    let chunked = transfer_encodings > 0;
    // Both framings at once is the classic smuggling vector; HTTP/1.0 has
    // no chunked framing at all.
    if chunked && (content_length.is_some() || transfer_encodings > 1 || req.version != Some(1)) {
        return Parsed::Invalid(400);
    }

    let target = req.path.unwrap_or("");
    let (path, query) = match target.find('?') {
        Some(i) => (&target[..i], &target[i + 1..]),
        None => (target, ""),
    };
    let body_start = at + head_len;
    let (len, total) = if chunked {
        match chunks(&buf[body_start..], body_limit::<A>(path)) {
            Chunks::Complete { wire, body } => (body, head_len + wire),
            Chunks::Partial => {
                return Parsed::Partial {
                    need: 0,
                    expect_continue,
                    body: true,
                };
            }
            Chunks::TooLarge => return Parsed::Invalid(413),
            Chunks::Invalid => return Parsed::Invalid(400),
        }
    } else {
        let len = content_length.unwrap_or(0);
        if len > 0 && len > body_limit::<A>(path) {
            return Parsed::Invalid(413);
        }
        if buf.len() - body_start < len {
            return Parsed::Partial {
                need: head_len + len,
                expect_continue,
                body: true,
            };
        }
        (len, head_len + len)
    };

    cx.method = Method::parse(req.method.unwrap_or(""));
    cx.http11 = req.version == Some(1);
    cx.path = Span::of(buf, path.as_bytes());
    cx.query = Span::of(buf, query.as_bytes());
    cx.body = Span {
        start: body_start as u32,
        len: len as u32,
    };
    if chunked {
        // The body's data moves up over the chunk framing, into one piece
        // where `cx.body` says. The head stays as it is.
        unchunk(&mut cx.buf[body_start..at + total]);
    }
    Parsed::Request {
        len: total,
        keep_alive,
    }
}

/// The largest body any route takes, whatever its limit says: requests are
/// described by 32-bit offsets into the read buffer.
const MAX_BODY: usize = 1 << 31;

/// The body limit for a request to `path`: its route's `BODY_LIMIT`, or
/// `WISP_BODY_LIMIT`, and at most `MAX_BODY`. Only requests with a body
/// look it up.
pub(crate) fn body_limit<A: App>(path: &str) -> usize {
    let mut segs = [""; MAX_SEGS];
    let route = if path.starts_with('/') {
        split(path, &mut segs).and_then(|n| A::route(path, &segs[..n]))
    } else {
        None
    };
    route
        .and_then(|(id, _)| A::body_limit(id))
        .unwrap_or(crate::settings().body_limit)
        .min(MAX_BODY)
}

enum Chunks {
    /// The body is `body` bytes, framed in `wire` bytes.
    Complete {
        wire: usize,
        body: usize,
    },
    Partial,
    TooLarge,
    Invalid,
}

/// Reads a chunked body (RFC 9112 §7.1) from the start of `b`, without
/// changing it. Strict: hex sizes of at most 16 digits, CRLF line ends,
/// extensions and trailers skipped but bounded, so that framing overhead
/// cannot make a body far larger on the wire than `limit`.
fn chunks(b: &[u8], limit: usize) -> Chunks {
    // A line ending in CRLF from `from`: the index of its CR.
    let line = |from: usize| -> Result<Option<usize>, ()> {
        for (i, &c) in b.iter().enumerate().skip(from).take(MAX_HEAD) {
            match c {
                b'\r' if b.get(i + 1) == Some(&b'\n') => return Ok(Some(i)),
                b'\r' if i + 1 == b.len() => return Ok(None),
                b'\r' | b'\n' => return Err(()),
                _ => {}
            }
        }
        if b.len() - from.min(b.len()) > MAX_HEAD {
            Err(())
        } else {
            Ok(None)
        }
    };
    let (mut i, mut body) = (0usize, 0usize);
    loop {
        let end = match line(i) {
            Ok(Some(end)) => end,
            Ok(None) => return Chunks::Partial,
            Err(()) => return Chunks::Invalid,
        };
        let digits = b[i..end]
            .iter()
            .take_while(|c| c.is_ascii_hexdigit())
            .count();
        let rest = &b[i + digits..end];
        if digits == 0
            || digits > 16
            || !(rest.is_empty() || rest.trim_ascii_start().starts_with(b";"))
        {
            return Chunks::Invalid;
        }
        let Ok(size) = usize::try_from(hex(&b[i..i + digits])) else {
            return Chunks::TooLarge;
        };
        i = end + 2;
        if size == 0 {
            // Trailer fields, up to an empty line.
            let trailers = i;
            loop {
                match line(i) {
                    Ok(Some(end)) if end == i => return Chunks::Complete { wire: i + 2, body },
                    Ok(Some(end)) => i = end + 2,
                    Ok(None) => return Chunks::Partial,
                    Err(()) => return Chunks::Invalid,
                }
                if i - trailers > MAX_HEAD {
                    return Chunks::Invalid;
                }
            }
        }
        body = match body.checked_add(size) {
            Some(n) if n <= limit => n,
            _ => return Chunks::TooLarge,
        };
        // Framing may at most double the body, plus a little.
        if i > body.saturating_mul(2).saturating_add(MAX_HEAD) {
            return Chunks::TooLarge;
        }
        if b.len() - i < size + 2 {
            return Chunks::Partial;
        }
        if &b[i + size..i + size + 2] != b"\r\n" {
            return Chunks::Invalid;
        }
        i += size + 2;
    }
}

/// Moves the data of the chunked body `b`, which [`chunks`] found complete,
/// to its start.
fn unchunk(b: &mut [u8]) {
    let (mut i, mut w) = (0, 0);
    loop {
        let digits = b[i..].iter().take_while(|c| c.is_ascii_hexdigit()).count();
        let size = hex(&b[i..i + digits]) as usize;
        i += b[i..].windows(2).position(|p| p == b"\r\n").unwrap_or(0) + 2;
        if size == 0 {
            return;
        }
        b.copy_within(i..i + size, w);
        w += size;
        i += size + 2;
    }
}

/// At most 16 hex digits, so it fits.
fn hex(digits: &[u8]) -> u64 {
    digits.iter().fold(0, |n, &c| {
        n << 4 | (c as char).to_digit(16).unwrap_or(0) as u64
    })
}

/// A response, decided but not yet written: what every host sends, the
/// built-in server, [`handle`], tower. The host adds `content-length`,
/// `date` and `connection` as its protocol needs.
pub struct Reply {
    pub status: u16,
    /// `content-type` first, when there is a body.
    pub headers: Vec<(Cow<'static, str>, Cow<'static, str>)>,
    pub body: Body,
}

pub enum Body {
    Bytes(Vec<u8>),
    Static(&'static [u8]),
    /// The page in the request's `Out`, inside the app's shell. Only the
    /// built-in server sees it; [`handle`] renders it to `Bytes`.
    #[doc(hidden)]
    Page,
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
    fn set(&mut self, status: u16, content_type: &'static str, body: Body) {
        self.status = status;
        self.headers.clear();
        self.headers
            .push((Cow::Borrowed("content-type"), Cow::Borrowed(content_type)));
        self.body = body;
    }

    fn set_plain(&mut self, status: u16, text: &'static str) {
        self.set(
            status,
            "text/plain; charset=utf-8",
            Body::Static(text.as_bytes()),
        );
    }

    fn add(&mut self, headers: &mut Vec<(Cow<'static, str>, String)>) {
        self.headers
            .extend(headers.drain(..).map(|(n, v)| (n, Cow::Owned(v))));
    }

    /// The upgrade of a [`Response::websocket`], taken out of the body.
    fn take_websocket(&mut self) -> Option<crate::ws::Upgrade> {
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
    pub method: String,
    /// Path and query: `/posts?page=2`.
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// The client's address. Loopback unless set.
    pub peer: SocketAddr,
}

impl Request {
    pub fn new(method: &str, target: &str) -> Request {
        Request {
            method: method.into(),
            target: target.into(),
            headers: Vec::new(),
            body: Vec::new(),
            peer: SocketAddr::from(([127, 0, 0, 1], 0)),
        }
    }

    pub fn header(&mut self, name: &str, value: &str) {
        self.headers.push((name.into(), value.into()));
    }
}

/// Answers one request in process, with no socket: the same parser, limits,
/// routing, hooks and error pages as the built-in server. Run
/// [`crate::prepare`] once first, so `init` has run.
pub async fn handle<A: App>(req: Request) -> Reply {
    let headers = req.headers.iter().map(|(n, v)| (n.as_str(), v.as_bytes()));
    match Cx::from_request::<A>(&req.method, &req.target, headers, &req.body, req.peer) {
        Ok(cx) => answer::<A>(cx).await,
        Err(status) => Reply::plain(status),
    }
}

/// [`decide`] for a request of its own, with the page rendered.
pub(crate) async fn answer<A: App>(mut cx: Cx) -> Reply {
    setup::<A>();
    let mut out = Out::default();
    let mut reply = Reply::default();
    decide::<A>(&mut cx, &mut out, &mut reply).await;
    if let Body::WebSocket(_) = reply.body {
        reply.set_plain(501, "WebSockets need Wisp's own server");
    }
    if let Body::Page = reply.body {
        let live = out.live.tail();
        reply.body = Body::Bytes(page::<A>(&out, &live).concat().into_bytes());
    }
    // HEAD gets the headers of a GET and no body; 204 and 304 have none.
    let bodiless = bodiless(reply.status);
    if cx.method == Method::Head || bodiless {
        let len = match &reply.body {
            Body::Bytes(b) => Some(b.len()),
            Body::Static(b) => Some(b.len()),
            Body::Page | Body::Stream(_) | Body::WebSocket(_) => None,
        };
        reply.body = Body::Static(b"");
        if let Some(len) = len.filter(|_| reply.header("content-length").is_none() && !bodiless) {
            reply
                .headers
                .push((Cow::Borrowed("content-length"), Cow::Owned(len.to_string())));
        }
    }
    // As `serialize` does on the wire: a header that would split the
    // response is left out, and the framing is the host's (only an answer
    // to HEAD may give its length).
    let head = cx.method == Method::Head;
    reply.headers.retain(|(n, v)| {
        valid_header(n, v)
            && !n.eq_ignore_ascii_case("transfer-encoding")
            && (head || !n.eq_ignore_ascii_case("content-length"))
    });
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
        // What would end a line or a field here would let one request pass
        // for two.
        let bad = |s: &[u8]| s.iter().any(|&b| b == b'\r' || b == b'\n' || b == 0);
        if method.is_empty()
            || !method.bytes().all(|b| b.is_ascii_alphabetic())
            || target.bytes().any(|b| b <= b' ' || b == 0x7f)
        {
            return Err(400);
        }
        let mut cx = Cx::new(peer);
        let buf = &mut cx.buf;
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
        match parse::<A>(&mut cx, 0) {
            Parsed::Request { len, .. } if len == cx.buf.len() => Ok(cx),
            Parsed::Invalid(status) => Err(status),
            _ => Err(400),
        }
    }
}

/// Decides the response to the request in `cx`, and gives it `x-request-id`
/// when the request has an id (`WISP_REQUEST_ID=on`, or `cx.request_id()`).
async fn decide<A: App>(cx: &mut Cx, out: &mut Out, reply: &mut Reply) {
    if crate::settings().request_id {
        cx.request_id();
    }
    match crate::idem::start(cx) {
        None => decide_inner::<A>(cx, out, reply).await,
        Some(Start::Fresh(key)) => {
            decide_inner::<A>(cx, out, reply).await;
            crate::idem::finish(key, reply);
        }
        Some(Start::Replay(r)) => *reply = r,
        Some(Start::Refused(e)) => {
            let body = e.json(e.message(), false).into_bytes();
            reply.set(e.status, "application/json", Body::Bytes(body));
        }
    }
    if let Some(id) = cx.id() {
        reply
            .headers
            .push((Cow::Borrowed("x-request-id"), Cow::Owned(id.to_string())));
    }
}

/// Static files, routing, hooks, redirects, error pages. Writes nothing;
/// see [`serialize`].
async fn decide_inner<A: App>(cx: &mut Cx, out: &mut Out, reply: &mut Reply) {
    // Only dev builds log request timing. `then` skips the clock read
    // itself when `cfg!` is false, so a release build has no cost here.
    let started =
        cfg!(all(debug_assertions, not(target_arch = "wasm32"))).then(std::time::Instant::now);
    let method = cx.method;

    let path = cx.path();
    if !path.starts_with('/') {
        return reply.set_plain(400, "Bad Request");
    }
    if matches!(
        path,
        "/_wisp/openapi.json" | "/_wisp/docs" | "/_wisp/client.ts"
    ) && matches!(method, Method::Get | Method::Head)
        && crate::settings().api_docs
        && !A::openapi().is_empty()
    {
        return match path {
            "/_wisp/docs" => reply.set(200, "text/html; charset=utf-8", Body::Static(api_docs())),
            "/_wisp/client.ts" => reply.set(
                200,
                "text/plain; charset=utf-8",
                Body::Static(A::client_ts().as_bytes()),
            ),
            _ => reply.set(
                200,
                "application/json",
                Body::Static(A::openapi().as_bytes()),
            ),
        };
    }
    if cfg!(debug_assertions) && path.starts_with("/_wisp/") {
        let (status, msg) = dev::endpoint::<A>(method, path, cx.body(), cx.peer());
        return reply.set_plain(status, msg);
    }
    if matches!(method, Method::Get | Method::Head) && file::<A>(cx, path, reply) {
        return;
    }
    if path.len() > 1 && path.ends_with('/') {
        // One leading slash: `//evil.example/` would send the browser to
        // another site (and so would `/\evil.example/`).
        let trimmed = path.trim_matches(['/', '\\']);
        let query = cx.query_string();
        let location = if query.is_empty() {
            format!("/{trimmed}")
        } else {
            format!("/{trimmed}?{query}")
        };
        reply.set_plain(308, "");
        reply
            .headers
            .push((Cow::Borrowed("location"), Cow::Owned(location)));
        return;
    }

    // Route, then turn the matched parameters into spans so `cx` can be
    // handed out mutably.
    let mut segs = [""; MAX_SEGS];
    let route = split(path, &mut segs)
        .and_then(|n| A::route(path, &segs[..n]))
        .map(|(id, raw)| (id, raw.map(|s| Span::of(&cx.buf, s.as_bytes()))));
    out.clear();
    if let Some((id, params)) = route {
        cx.set_params(A::PARAMS[id], params);
    }
    let mut result = catch(A::handle(route.map(|(id, _)| id), cx, out)).await;
    if result.is_ok()
        && let Some(res) = out.response.as_mut().filter(|r| r.upgrade.is_some())
    {
        result = crate::ws::handshake(cx).map(|accept| {
            res.headers
                .push((Cow::Borrowed("sec-websocket-accept"), accept));
        });
    }

    // What went wrong in a 5xx, for the log. The page may say less.
    let mut failure = None;
    match result {
        Ok(()) => match out.response.take() {
            Some(mut res) => {
                reply.status = res.status;
                reply.headers.clear();
                if !res.content_type.is_empty() {
                    reply
                        .headers
                        .push((Cow::Borrowed("content-type"), res.content_type));
                }
                reply.add(&mut res.headers);
                reply.body = match (res.upgrade.take(), res.stream.take()) {
                    (Some(upgrade), _) => Body::WebSocket(upgrade),
                    (None, Some(body)) => Body::Stream(body),
                    (None, None) => Body::Bytes(res.body),
                };
            }
            None => reply.set(cx.status, "text/html; charset=utf-8", Body::Page),
        },
        Err(e) if e.status < 400 => {
            // Redirect. Headers set before it (a login cookie) still apply.
            // wisp.js gets it as `x-wisp-location` and goes there itself:
            // fetch would follow it with the post's own headers, and to
            // another site (a payment page) not at all.
            let js = cx.header("x-wisp").is_some();
            reply.set_plain(if js { 200 } else { e.status }, "");
            if let Some((name, value)) = e.header.map(|h| *h) {
                reply.headers.push((
                    Cow::Borrowed(if js && name == "location" {
                        "x-wisp-location"
                    } else {
                        name
                    }),
                    Cow::Owned(value),
                ));
            }
        }
        Err(mut e) => {
            if e.status >= 500 {
                failure = Some(e.detail());
            }
            // 5xx details can leak internals; only dev builds show them. An
            // error that says no more than its status's name gets a sentence
            // about the status instead.
            let message = if (e.status >= 500 && !cfg!(debug_assertions))
                || e.message.is_empty()
                || e.message.eq_ignore_ascii_case(reason(e.status))
            {
                sentence(e.status)
            } else {
                e.message()
            };
            out.clear();
            // The headers of the page that failed go with it; the `before`
            // hook's stay.
            cx.out_headers.truncate(cx.kept_headers);
            if wants_json(cx) {
                let problem = crate::settings().problem_json
                    || cx
                        .header("accept")
                        .is_some_and(|a| a.contains("application/problem+json"));
                let body = e.json(message, problem).into_bytes();
                let kind = match problem {
                    true => "application/problem+json",
                    false => "application/json",
                };
                reply.set(e.status, kind, Body::Bytes(body));
            } else {
                let rendered = catch(A::error(
                    route.map(|(id, _)| id),
                    cx,
                    out,
                    e.status,
                    message,
                ))
                .await;
                if rendered.is_err() || out.response.is_some() {
                    out.clear();
                    rt::default_error(cx, out, e.status, message);
                }
                reply.set(e.status, "text/html; charset=utf-8", Body::Page);
            }
            if let Some((name, value)) = e.header.take().map(|h| *h) {
                reply.headers.push((Cow::Borrowed(name), Cow::Owned(value)));
            }
        }
    }
    reply.add(&mut cx.out_headers);

    if cfg!(debug_assertions) {
        let blocked = Some(BLOCKED.replace(Duration::ZERO)).filter(|&b| b >= BLOCKING);
        dev::log_request(
            method.as_str(),
            cx.path(),
            reply.status,
            started.unwrap().elapsed(),
            failure.as_deref(),
            blocked,
            cx.id(),
        );
    } else if let Some(f) = failure {
        let id = cx.id().map_or(String::new(), |id| format!(" [{id}]"));
        log(format_args!(
            "wisp: {} {} {}{id}: {f}",
            reply.status,
            method.as_str(),
            cx.path()
        ));
    }
}

/// Whether an error goes back as JSON rather than an error page: a request
/// under `/api`, one that sent JSON, one that asks for JSON and not HTML,
/// or one to a `+server.rs` endpoint from anything but a browser page.
fn wants_json(cx: &Cx) -> bool {
    let path = cx.path();
    let accept = cx.header("accept").unwrap_or("");
    path == "/api"
        || path.starts_with("/api/")
        || crate::input::is_json(cx)
        || (accept.contains("json") && !accept.contains("text/html"))
        || (cx.api && !accept.contains("text/html"))
}

/// A status whose response has no body, and no `content-length` (RFC 9110
/// §8.6, §15): informational, 204 and 304. A body the app gave one is dropped
/// rather than sent where the client does not expect it.
fn bodiless(status: u16) -> bool {
    status < 200 || status == 204 || status == 304
}

/// A response whose body is still being made, for the connection to send.
struct Streamed {
    body: mpsc::Receiver<Vec<u8>>,
    /// Framed in chunks (HTTP/1.1); otherwise the body ends when the
    /// connection closes.
    chunked: bool,
    /// The connection closes after it.
    close: bool,
}

/// Writes `reply` as HTTP/1.1 and leaves it empty. A streamed body is
/// returned for the connection to send as it comes.
fn serialize<A: App>(
    w: &mut Vec<u8>,
    reply: &mut Reply,
    out: &Out,
    http11: bool,
    keep_alive: bool,
    head_only: bool,
) -> Option<Streamed> {
    let bodiless = bodiless(reply.status);
    let stream = matches!(reply.body, Body::Stream(_)) && !bodiless;
    // HTTP/1.0 has no chunks: a streamed body ends with the connection.
    let chunked = stream && http11;
    let keep_alive = keep_alive && (!stream || chunked || head_only);
    // Browser code, if the page has any, ends the body.
    let live = if let Body::Page = reply.body {
        out.live.tail()
    } else {
        String::new()
    };
    let parts = page::<A>(out, &live);
    let len = match &reply.body {
        Body::Bytes(b) => b.len(),
        Body::Static(b) => b.len(),
        Body::Page => parts.iter().map(|p| p.len()).sum(),
        Body::Stream(_) | Body::WebSocket(_) => 0,
    };

    // The framing is the server's: an app's own `content-length` or
    // `transfer-encoding` (copied from another server's response, say) would
    // contradict it, and the client would read the next response wrong. Only
    // an answer to HEAD, which has no body to count, may give its length.
    let own_length = head_only && reply.header("content-length").is_some();
    status_line(w, reply.status);
    if chunked {
        w.extend_from_slice(b"transfer-encoding: chunked\r\n");
    } else if !stream && !bodiless && !own_length {
        w.extend_from_slice(b"content-length: ");
        push_decimal(w, len as u64);
        w.extend_from_slice(b"\r\n");
    }
    date(w);
    if !keep_alive {
        w.extend_from_slice(b"connection: close\r\n");
    } else if !http11 {
        // HTTP/1.0 closes after each response unless told otherwise.
        w.extend_from_slice(b"connection: keep-alive\r\n");
    }
    for (name, value) in &reply.headers {
        let framing = match name.len() {
            14 => !own_length && name.eq_ignore_ascii_case("content-length"),
            17 => name.eq_ignore_ascii_case("transfer-encoding"),
            _ => false,
        };
        if !framing {
            header(w, name, value);
        }
    }
    w.extend_from_slice(b"\r\n");

    let body = std::mem::replace(&mut reply.body, Body::Static(b""));
    reply.headers.clear();
    if head_only || bodiless {
        return None;
    }
    match body {
        Body::Bytes(b) => w.extend_from_slice(&b),
        Body::Static(b) => w.extend_from_slice(b),
        Body::WebSocket(_) => {} // taken out before; a 101 has no body
        Body::Page => {
            w.reserve(len);
            for part in parts {
                w.extend_from_slice(part.as_bytes());
            }
        }
        Body::Stream(body) => {
            return Some(Streamed {
                body,
                chunked,
                close: !keep_alive,
            });
        }
    }
    None
}

/// The parts of a page, in order: the shell around the tags for
/// `%wisp.head%`, the page's head and body, and its browser code.
fn page<'a, A: App>(out: &'a Out, live: &'a str) -> [&'a str; 7] {
    let [s0, s1, s2] = A::shell();
    let tags = HEAD_TAGS.get().map_or("", String::as_str);
    [s0, tags, &out.head, s1, &out.body, live, s2]
}

thread_local! {
    /// Set while `catch` polls a handler. A panic there is reported once, as
    /// the request's 500, rather than by the panic hook as well.
    static IN_HANDLER: Cell<bool> = const { Cell::new(false) };
    /// Where the handler that just panicked was, for the 500's message.
    static PANICKED_AT: Cell<Option<String>> = const { Cell::new(None) };
    /// Dev builds: the longest a handler of this request held the thread in
    /// one poll.
    static BLOCKED: Cell<Duration> = const { Cell::new(Duration::ZERO) };
}

/// Quiets the panic hook for handler panics, which `catch` reports with the
/// request, and remembers where each happened. Any other panic, or any at
/// all with `RUST_BACKTRACE` set, still reaches the hook that was there.
fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if IN_HANDLER.get() {
                if let Some(l) = info.location() {
                    PANICKED_AT.set(Some(format!(
                        "{}:{}:{}",
                        short_path(l.file()),
                        l.line(),
                        l.column()
                    )));
                }
                if std::env::var_os("RUST_BACKTRACE").is_none() {
                    return;
                }
            }
            previous(info);
        }));
    });
}

/// `file` relative to the working directory, with `/` separators, when it is
/// inside it: `src/routes/+page.rs` rather than the whole path the
/// generated code gave the compiler.
fn short_path(file: &str) -> String {
    let rel = std::env::current_dir()
        .ok()
        .and_then(|d| Path::new(file).strip_prefix(d).ok().map(Path::to_path_buf));
    match rel {
        Some(r) => r.to_string_lossy().replace('\\', "/"),
        None => file.to_string(),
    }
}

/// Runs a handler future, turning a panic into a 500 so one bad request
/// cannot take the connection (or anything else) down with it.
pub(crate) async fn catch<F: Future<Output = crate::Result<()>>>(f: F) -> crate::Result<()> {
    let mut f = std::pin::pin!(f);
    std::future::poll_fn(move |cx| {
        let began = cfg!(all(debug_assertions, not(target_arch = "wasm32"))).then(Instant::now);
        IN_HANDLER.set(true);
        let polled = catch_unwind(AssertUnwindSafe(|| f.as_mut().poll(cx)));
        IN_HANDLER.set(false);
        if let Some(began) = began {
            BLOCKED.set(BLOCKED.get().max(began.elapsed()));
        }
        match polled {
            Ok(poll) => poll,
            Err(panic) => {
                let msg = panic
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "panic".into());
                let msg = match PANICKED_AT.take() {
                    Some(at) => format!("panic at {at}: {msg}"),
                    None => format!("panic: {msg}"),
                };
                Poll::Ready(Err(Error::new(500, msg)))
            }
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

/// Whether a route matches `path`.
fn routed<A: App>(path: &str) -> bool {
    let mut segs = [""; MAX_SEGS];
    split(path, &mut segs)
        .and_then(|n| A::route(path, &segs[..n]))
        .is_some()
}

/// Static files: the client script, then embedded assets (release) or
/// files on disk (dev). Returns false if the path is not a file.
fn file<A: App>(cx: &Cx, path: &str, reply: &mut Reply) -> bool {
    // Looked up only once a file matched: most requests are for pages.
    let versioned = || cx.query_string().split('&').any(|kv| kv.starts_with("v="));
    let inm = || cx.header("if-none-match");
    if path == "/_app/wisp.js" {
        send_file(
            reply,
            Body::Static(CLIENT_JS),
            "js",
            Some(CLIENT_JS_ETAG),
            versioned(),
            inm(),
        );
        return true;
    }
    if path == "/_app/live.js" {
        send_file(
            reply,
            Body::Static(LIVE_JS),
            "js",
            Some(CLIENT_JS_ETAG),
            versioned(),
            inm(),
        );
        return true;
    }
    // Templates' browser modules, compiled in: dev builds too, since a
    // change to one is a rebuild anyway.
    if path.starts_with("/_app/c/")
        && let Some(m) = A::client_module(path)
    {
        send_file(
            reply,
            Body::Static(m.source.as_bytes()),
            "js",
            Some(m.etag),
            versioned(),
            inm(),
        );
        return true;
    }
    if cfg!(debug_assertions) {
        let dev_file = match path {
            "/_app/wisp-dev.js" => Some((DEV_JS, "js")),
            "/_app/wisp-ui.css" => Some((UI_CSS.as_bytes(), "css")),
            "/_app/wisp-dialog.css" => Some((DIALOG_CSS, "css")),
            _ => None,
        };
        if let Some((body, ext)) = dev_file {
            send_file(reply, Body::Static(body), ext, None, false, inm());
            return true;
        }
    }
    if cfg!(debug_assertions) {
        // A page's path goes to the disk only if `static/` had a file there.
        if path != "/_app/app.css"
            && routed::<A>(path)
            && !dev::listed(A::ROOT, &decode(path.as_bytes(), false))
        {
            return false;
        }
        let Some((bytes, ext)) = dev::read_file(A::ROOT, path) else {
            return false;
        };
        send_file(reply, Body::Bytes(bytes), &ext, None, false, inm());
        return true;
    }
    let Some(a) = A::asset(path) else {
        return false;
    };
    send_file(
        reply,
        Body::Static(a.body),
        a.ext,
        Some(a.etag),
        versioned(),
        inm(),
    );
    true
}

fn send_file(
    reply: &mut Reply,
    body: Body,
    ext: &str,
    etag: Option<&'static str>,
    versioned: bool,
    if_none_match: Option<&str>,
) {
    let cache = match etag {
        None => "no-store",
        Some(_) if versioned => "public, max-age=31536000, immutable",
        Some(_) => "public, max-age=0, must-revalidate",
    };
    if etag.is_some() && if_none_match == etag {
        reply.set(304, "", Body::Static(b""));
        reply.headers.clear(); // a 304 describes the file it did not send
    } else {
        reply.set(200, mime(ext), body);
    }
    if let Some(tag) = etag {
        reply
            .headers
            .push((Cow::Borrowed("etag"), Cow::Borrowed(tag)));
    }
    reply
        .headers
        .push((Cow::Borrowed("cache-control"), Cow::Borrowed(cache)));
}

/// Sends a streamed body as it is made, until its sender is dropped or the
/// server stops (both end it properly), or the client goes (an error).
/// Anything the client sends meanwhile is kept in `early`, up to a limit.
#[cfg(not(target_arch = "wasm32"))]
async fn pump(
    stream: &mut TcpStream,
    mut body: mpsc::Receiver<Vec<u8>>,
    chunked: bool,
    w: &mut Vec<u8>,
    early: &mut Vec<u8>,
) -> io::Result<()> {
    enum Next {
        Chunk(Option<Vec<u8>>),
        Stop,
        Gone,
    }
    loop {
        let next = first(
            async { Next::Chunk(body.recv().await) },
            first(
                async {
                    stopped().await;
                    Next::Stop
                },
                async {
                    loop {
                        if early.len() >= KEEP_CAPACITY {
                            std::future::pending::<()>().await;
                        }
                        match stream.read_buf(early).await {
                            Ok(0) | Err(_) => return Next::Gone,
                            Ok(_) => {}
                        }
                    }
                },
            ),
        )
        .await;
        w.clear();
        match next {
            Next::Chunk(Some(chunk)) if chunk.is_empty() => continue, // would read as the end
            Next::Chunk(Some(chunk)) if chunked => {
                w.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
                w.extend_from_slice(&chunk);
                w.extend_from_slice(b"\r\n");
            }
            Next::Chunk(Some(chunk)) => w.extend_from_slice(&chunk),
            Next::Chunk(None) | Next::Stop => {
                if chunked {
                    write(stream, b"0\r\n\r\n").await?;
                }
                return Ok(());
            }
            Next::Gone => return Err(io::ErrorKind::ConnectionAborted.into()),
        }
        write(stream, w).await?;
    }
}

fn status_line(w: &mut Vec<u8>, status: u16) {
    w.extend_from_slice(b"HTTP/1.1 ");
    push_decimal(w, status as u64);
    w.push(b' ');
    w.extend_from_slice(reason(status).as_bytes());
    w.extend_from_slice(b"\r\n");
}

/// A header the app set with a line break or NUL in it (through `Response`'s
/// public fields, which nothing checks) would split the response: it is
/// left out.
fn header(w: &mut Vec<u8>, name: &str, value: &str) {
    if !valid_header(name, value) {
        return;
    }
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
    s.iter().try_fold(0usize, |n, &b| {
        let digit = b.checked_sub(b'0').filter(|&d| d < 10)?;
        n.checked_mul(10)?.checked_add(digit as usize)
    })
}

// The time, kept by a background thread that ticks once a second, so the
// request path reads an atomic instead of the OS clock (two clock reads were
// ~2% of a plaintext request). Whole seconds are all it needs: the Date
// header has no finer resolution, and timeouts are swept once a second. The
// same tradeoff nginx makes with its cached time.

/// The current unix second, for the Date header.
static NOW: AtomicU64 = AtomicU64::new(0);
/// Seconds since the clock started, from the monotonic clock: what request
/// deadlines are counted in.
static SECONDS: AtomicU64 = AtomicU64::new(0);

pub(crate) fn seconds() -> u64 {
    SECONDS.load(Ordering::Relaxed)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// When `seconds()` was 0.
static START: OnceLock<Instant> = OnceLock::new();

/// The instant of `deadline`, in `seconds()`, for a tokio timer.
#[cfg(not(target_arch = "wasm32"))]
fn instant(deadline: u64) -> tokio::time::Instant {
    let start = *START.get_or_init(Instant::now);
    tokio::time::Instant::from_std(start + Duration::from_secs(deadline))
}

fn start_clock() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        NOW.store(unix_now(), Ordering::Relaxed);
        let start = *START.get_or_init(Instant::now);
        std::thread::Builder::new()
            .name("wisp-clock".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                    NOW.store(unix_now(), Ordering::Relaxed);
                    SECONDS.store(start.elapsed().as_secs(), Ordering::Relaxed);
                }
            })
            .expect("failed to start clock thread");
    });
}

thread_local! {
    /// (unix second, formatted date) — reformatted at most once a second.
    static DATE: Cell<(u64, [u8; 29])> = const { Cell::new((u64::MAX, [0; 29])) };
}

fn date(w: &mut Vec<u8>) {
    let now = NOW.load(Ordering::Relaxed);
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
    const MONTHS: [&[u8; 3]; 12] = [
        b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct", b"Nov",
        b"Dec",
    ];
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
        101 => "Switching Protocols",
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
        426 => "Upgrade Required",
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

pub(crate) fn mime(ext: &str) -> &'static str {
    match ext {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "webmanifest" => "application/manifest+json",
        "txt" => "text/plain; charset=utf-8",
        "csv" => "text/csv",
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
    stays_inside(rel).then(|| rel.to_string())
}

/// Whether `rel`, joined to a folder, names something inside it: no `..`,
/// no empty segment, nothing a drive or an absolute path could use.
pub(crate) fn stays_inside(rel: &str) -> bool {
    rel.split('/')
        .all(|s| !s.is_empty() && s != "." && s != ".." && !s.contains(['\\', ':', '\0']))
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
    fn headers_that_would_split_a_response_are_left_out() {
        let mut w = Vec::new();
        header(&mut w, "x-a", "1");
        header(&mut w, "x-b", "2\r\nset-cookie: x=1");
        header(&mut w, "x-c\n", "3");
        header(&mut w, "x-d", "4\0");
        assert_eq!(w, b"x-a: 1\r\n");
    }

    #[test]
    fn paths() {
        let mut segs = [""; MAX_SEGS];
        assert_eq!(split("/", &mut segs), Some(0));
        assert_eq!(split("/a/b", &mut segs), Some(2));
        assert_eq!(&segs[..2], ["a", "b"]);
        assert_eq!(split(&"/x".repeat(40), &mut segs), None);
        assert_eq!(
            safe_relative_path("/img/a%20b.png").as_deref(),
            Some("img/a b.png")
        );
        for bad in [
            "/../secret",
            "/a/%2e%2e/b",
            "/a//b",
            "/",
            "/C:/x",
            "/a\\b",
            "/a/",
        ] {
            assert_eq!(safe_relative_path(bad), None, "{bad}");
        }
    }

    use crate::fuzz::{Fuzz, Rng, SMALL, mutate};

    /// A request as a client may send it, and what it should parse to.
    struct Sent {
        wire: Vec<u8>,
        path: &'static str,
        query: String,
        body: Vec<u8>,
        headers: usize,
    }

    fn sent(rng: &mut Rng) -> Sent {
        let method = rng.pick(&["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "HEAD"]);
        let path = rng.pick(&["/", "/small", "/p/a%20b", "/no/such/page"]);
        let n = rng.below(24);
        let query = String::from_utf8(rng.bytes(n, b"ab=&%2F+.")).unwrap();
        let mut wire = format!("{method} {path}");
        if !query.is_empty() || rng.one_in(4) {
            wire += "?";
            wire += &query;
        }
        wire += " HTTP/1.1\r\n";
        let mut headers = rng.below(12);
        for _ in 0..headers {
            let n = 1 + rng.below(10);
            let name = String::from_utf8(rng.bytes(n, b"abcxyz-_")).unwrap();
            let n = rng.below(40);
            let value = String::from_utf8(rng.bytes(n, b"abc 012=;,/\t")).unwrap();
            wire += &format!(
                "x-{name}:{}{}\r\n",
                rng.pick(&["", " ", "  "]),
                value.trim()
            );
        }
        let n = rng.pick(&[0, 1, 10, SMALL, SMALL + 1, 500]);
        let body = rng.bytes(n, b"");
        if rng.one_in(2) {
            headers += 1;
            wire += "transfer-encoding: chunked\r\n\r\n";
            let mut wire = wire.into_bytes();
            let mut left = &body[..];
            while !left.is_empty() {
                let size = (1 + rng.below(80)).min(left.len());
                let size_text = if rng.one_in(2) {
                    format!("{size:x}")
                } else {
                    format!("{size:X}")
                };
                wire.extend_from_slice(size_text.as_bytes());
                if rng.one_in(4) {
                    wire.extend_from_slice(b";ext=\"v\"");
                }
                wire.extend_from_slice(b"\r\n");
                wire.extend_from_slice(&left[..size]);
                wire.extend_from_slice(b"\r\n");
                left = &left[size..];
            }
            wire.extend_from_slice(b"0\r\n");
            if rng.one_in(3) {
                wire.extend_from_slice(b"x-trailer: 1\r\n");
            }
            wire.extend_from_slice(b"\r\n");
            return Sent {
                wire,
                path,
                query,
                body,
                headers,
            };
        }
        if !body.is_empty() || rng.one_in(3) {
            headers += 1;
            wire += &format!("Content-Length: {}\r\n", body.len());
        }
        wire += "\r\n";
        let mut wire = wire.into_bytes();
        wire.extend_from_slice(&body);
        Sent {
            wire,
            path,
            query,
            body,
            headers,
        }
    }

    fn limit(path: &str) -> usize {
        if path == "/small" {
            SMALL
        } else {
            crate::settings().body_limit
        }
    }

    fn cx_with(bytes: &[u8]) -> Cx {
        let mut cx = Cx::new(SocketAddr::from(([127, 0, 0, 1], 1)));
        cx.buf.extend_from_slice(bytes);
        cx
    }

    /// What a parsed request says must lie inside the buffer, within limits.
    fn check_parsed(cx: &Cx, at: usize, len: usize) {
        assert!(at + len <= cx.buf.len());
        let body = cx.body.range();
        assert!(body.start >= at && body.end <= at + len);
        assert!(body.len() <= limit(cx.path()));
        for (n, v) in &cx.headers {
            assert!(n.range().end <= at + len && v.range().end <= at + len);
        }
        let _ = (cx.query_string(), cx.headers().count(), cx.cookie("a"));
        let _ = (
            cx.form().iter().count(),
            cx.host(),
            cx.bearer(),
            cx.basic_auth(),
        );
    }

    #[test]
    fn requests_parse_whole_or_wait_for_more() {
        let mut rng = Rng::new(1);
        for _ in 0..3000 {
            let s = sent(&mut rng);
            let too_large = s.body.len() > limit(s.path);
            let mut cx = cx_with(&s.wire);
            match parse::<Fuzz>(&mut cx, 0) {
                Parsed::Request { len, .. } if !too_large => {
                    assert_eq!(len, s.wire.len());
                    assert_eq!(
                        (cx.path(), cx.query_string(), cx.body()),
                        (s.path, &*s.query, &s.body[..])
                    );
                    assert_eq!(cx.headers.len(), s.headers);
                    check_parsed(&cx, 0, len);
                }
                Parsed::Invalid(413) if too_large => {}
                _ => panic!("{:?}", String::from_utf8_lossy(&s.wire)),
            }
            // Any prefix is a request still arriving (or already too large).
            let cuts: Vec<usize> = if s.wire.len() < 200 {
                (0..s.wire.len()).collect()
            } else {
                (0..40).map(|_| rng.below(s.wire.len())).collect()
            };
            for cut in cuts {
                let mut cx = cx_with(&s.wire[..cut]);
                match parse::<Fuzz>(&mut cx, 0) {
                    Parsed::Partial { .. } => {}
                    Parsed::Invalid(413) if too_large => {}
                    _ => panic!("{cut} of {:?}", String::from_utf8_lossy(&s.wire)),
                }
            }
        }
    }

    #[test]
    fn pipelined_requests_parse_in_turn() {
        let mut rng = Rng::new(2);
        for _ in 0..1000 {
            let (a, b) = (sent(&mut rng), sent(&mut rng));
            if a.body.len() > limit(a.path) || b.body.len() > limit(b.path) {
                continue;
            }
            let mut cx = cx_with(&[&a.wire[..], &b.wire].concat());
            let Parsed::Request { len, .. } = parse::<Fuzz>(&mut cx, 0) else {
                panic!()
            };
            assert_eq!((len, cx.body()), (a.wire.len(), &a.body[..]));
            let Parsed::Request { len, .. } = parse::<Fuzz>(&mut cx, len) else {
                panic!()
            };
            assert_eq!((len, cx.body()), (b.wire.len(), &b.body[..]));
        }
    }

    #[test]
    fn broken_requests_never_panic() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let mut rng = Rng::new(3);
        let (mut out, mut reply, mut w) = (Out::default(), Reply::default(), Vec::new());
        for _ in 0..20_000 {
            let mut wire = sent(&mut rng).wire;
            mutate(&mut rng, &mut wire);
            let mut cx = cx_with(&wire);
            match parse::<Fuzz>(&mut cx, 0) {
                Parsed::Request { len, .. } => {
                    check_parsed(&cx, 0, len);
                    rt.block_on(decide::<Fuzz>(&mut cx, &mut out, &mut reply));
                    w.clear();
                    serialize::<Fuzz>(&mut w, &mut reply, &out, cx.http11, true, false);
                    assert!(w.starts_with(b"HTTP/1.1 "));
                }
                Parsed::Partial { need, .. } => assert!(need <= MAX_HEAD + MAX_BODY),
                Parsed::Invalid(status) => assert!(matches!(status, 400 | 413 | 431 | 501)),
            }
        }
    }

    #[test]
    fn heads_and_chunk_framing_are_bounded() {
        // A head that never ends is refused once it passes MAX_HEAD.
        let mut long = b"GET / HTTP/1.1\r\n".to_vec();
        while long.len() <= MAX_HEAD {
            long.extend_from_slice(b"x-a: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\r\n");
        }
        assert!(matches!(
            parse::<Fuzz>(&mut cx_with(&long), 0),
            Parsed::Invalid(431)
        ));
        let many: String = (0..MAX_HEADERS + 1)
            .map(|i| format!("x-{i}: 1\r\n"))
            .collect();
        let many = format!("GET / HTTP/1.1\r\n{many}\r\n");
        assert!(matches!(
            parse::<Fuzz>(&mut cx_with(many.as_bytes()), 0),
            Parsed::Invalid(431)
        ));

        let head = "POST /small HTTP/1.1\r\ntransfer-encoding: chunked\r\n\r\n";
        let body_of = |chunked: &str| {
            let mut cx = cx_with(format!("{head}{chunked}").as_bytes());
            match parse::<Fuzz>(&mut cx, 0) {
                Parsed::Request { .. } => Ok(cx.body().to_vec()),
                Parsed::Invalid(status) => Err(status),
                Parsed::Partial { .. } => Err(0),
            }
        };
        assert_eq!(
            body_of("3\r\nabc\r\n2;x=y\r\nde\r\n0\r\nt: 1\r\n\r\n"),
            Ok(b"abcde".to_vec())
        );
        assert_eq!(body_of("41\r\n"), Err(413), "over the route's limit");
        assert_eq!(body_of("ffffffffffffffff\r\n"), Err(413));
        assert_eq!(body_of("10000000000000000\r\n"), Err(400), "17 digits");
        assert_eq!(body_of("3\r\nabcX\r\n0\r\n\r\n"), Err(400));
        assert_eq!(body_of("3\nabc\r\n0\r\n\r\n"), Err(400), "bare LF");
        assert_eq!(body_of("-3\r\nabc\r\n0\r\n\r\n"), Err(400));
        assert_eq!(body_of("3\r\nabc\r\n0\r\n"), Err(0), "no end yet");
        // Framing that is most of the wire, with empty chunks, is refused.
        let padded = format!("1;{}\r\na\r\n", "e".repeat(MAX_HEAD - 8)).repeat(3);
        assert_eq!(body_of(&padded), Err(413));

        let mut rng = Rng::new(4);
        for _ in 0..20_000 {
            let n = rng.below(64);
            let b = rng.bytes(n, b"0123456789abcdefgxX;= \r\n");
            if let Chunks::Complete { wire, body } = chunks(&b, 1 << 20) {
                assert!(wire <= b.len() && body <= wire);
                let mut copy = b.clone();
                unchunk(&mut copy[..wire]);
            }
        }
    }

    #[test]
    fn slash_redirects_stay_on_the_site_and_framing_is_ours() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let (mut out, mut reply) = (Out::default(), Reply::default());
        for (path, to) in [
            ("//evil.example/", "/evil.example"),
            ("/\\evil.example/", "/evil.example"),
            ("/a/b//?x=1", "/a/b?x=1"),
            ("///", "/"),
        ] {
            let mut cx = cx_with(format!("GET {path} HTTP/1.1\r\n\r\n").as_bytes());
            assert!(matches!(parse::<Fuzz>(&mut cx, 0), Parsed::Request { .. }));
            rt.block_on(decide::<Fuzz>(&mut cx, &mut out, &mut reply));
            assert_eq!((reply.status, reply.header("location")), (308, Some(to)));
        }

        let mut w = Vec::new();
        let mut reply = Reply::plain(200);
        for (n, v) in [("content-length", "99"), ("Transfer-Encoding", "chunked")] {
            reply.headers.push((Cow::Borrowed(n), Cow::Borrowed(v)));
        }
        serialize::<Fuzz>(&mut w, &mut reply, &out, false, true, false);
        let text = String::from_utf8(w).unwrap().to_ascii_lowercase();
        assert!(text.contains("content-length: 2\r\n") && text.contains("connection: keep-alive"));
        assert!(!text.contains("99") && !text.contains("chunked"), "{text}");
    }

    #[test]
    fn a_request_id_is_echoed_when_asked_for() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let sent = |path: &str, id: &str| {
            let mut req = Request::new("GET", path);
            if !id.is_empty() {
                req.header("x-request-id", id);
            }
            rt.block_on(handle::<Fuzz>(req))
        };
        assert_eq!(sent("/p/id", "abc-1").header("x-request-id"), Some("abc-1"));
        assert_eq!(
            sent("/p/id", "").header("x-request-id").map(str::len),
            Some(16)
        );
        assert_eq!(sent("/p/other", "abc-1").header("x-request-id"), None);
    }

    #[test]
    fn requests_from_other_hosts_never_panic() {
        let mut rng = Rng::new(5);
        for _ in 0..5000 {
            let method = String::from_utf8(rng.upto(8, b"GETPOSt \r")).unwrap();
            let n = rng.below(30);
            let target =
                String::from_utf8_lossy(&rng.bytes(n, b"/a?=%& \x7f\xc3\xa9\r\n")).into_owned();
            let name = rng.text(8);
            let value = rng.upto(20, b"");
            let headers = [(name.as_str(), &value[..])];
            let body = rng.upto(100, b"");
            if let Ok(cx) =
                Cx::from_request::<Fuzz>(&method, &target, headers, &body, cx_with(b"").peer())
            {
                assert_eq!(cx.body(), body);
                check_parsed(&cx, 0, cx.buf.len());
            }
        }
    }
}
