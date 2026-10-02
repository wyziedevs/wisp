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

use crate::cx::{Cx, Known, Method, Span, decode, hex_digit, valid_header};
#[cfg(not(target_arch = "wasm32"))]
use crate::policy::READ_CAPACITY;
use crate::policy::{self, KEEP_CAPACITY, WRITE_TIMEOUT};
use crate::{App, Error, Out, dev, rt, swar};
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
use std::time::{Duration, Instant};
#[cfg(not(target_arch = "wasm32"))]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(not(target_arch = "wasm32"))]
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, mpsc};

/// Headers a request may have: as many as hyper's, room for a browser's
/// behind a proxy or two that add their own.
const MAX_HEADERS: usize = 100;
const MAX_HEAD: usize = 16 * 1024;
/// Time a stopping server waits for the requests under way.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);
/// A handler that holds its thread longer than this in one go is reported
/// in dev: every other connection on that thread waited meanwhile.
const BLOCKING: Duration = Duration::from_millis(100);

/// The browser runtime: as written in dev builds, without comments and
/// indentation in release ones (see `build.rs`). The ETag tells them apart.
#[cfg(debug_assertions)]
const CLIENT_JS: &[u8] = wisp_shared::WISP_JS.as_bytes();
#[cfg(not(debug_assertions))]
const CLIENT_JS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/wisp.js"));
#[cfg(debug_assertions)]
const CLIENT_JS_ETAG: &str = concat!("\"", env!("WISP_RUNTIME_V"), "-dev\"");
#[cfg(not(debug_assertions))]
const CLIENT_JS_ETAG: &str = concat!("\"", env!("WISP_RUNTIME_V"), "\"");
/// The runtime of client scripts and directives, linked by pages that
/// render any (see `live.rs`). Versioned like `wisp.js`.
#[cfg(debug_assertions)]
const LIVE_JS: &[u8] = wisp_shared::LIVE_JS.as_bytes();
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
/// Baked pages have them as wisp-build writes them (`Project::baked`), the
/// same out of dev mode: change both.
static HEAD_TAGS: OnceLock<String> = OnceLock::new();

/// Thread per core: `threads` workers, each a single-threaded tokio runtime
/// with its own I/O driver and timers. A connection stays on one thread for
/// its whole life, so the request path never wakes another thread or shares
/// a driver. (A multi-threaded tokio runtime funnels every socket event
/// through one driver; it measured at under half the throughput with cores
/// left idle.) On Linux the workers accept for themselves ([`run_linux`]);
/// elsewhere this thread accepts and hands each connection to the worker
/// with the fewest ([`run_tokio`]).
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

    let listener = bind_after_exit(addr)?;
    #[cfg(target_os = "linux")]
    return run_linux::<A>(&main, listener, threads.max(1));
    #[cfg(not(target_os = "linux"))]
    run_tokio::<A>(&main, listener, threads.max(1))
}

/// [`run`] where this thread accepts, on tokio's sockets.
#[cfg(not(any(target_arch = "wasm32", target_os = "linux")))]
fn run_tokio<A: App>(
    main: &tokio::runtime::Runtime,
    listener: std::net::TcpListener,
    threads: usize,
) -> io::Result<()> {
    listener.set_nonblocking(true)?;
    let workers = (0..threads)
        .map(|i| worker(i, std::future::pending))
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
                        if let Ok(s) = stream.into_std() {
                            refuse(&s);
                        }
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
                            connection::<A>(Conn::Tcp(stream), peer).await;
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

/// [`run`] on Linux: each worker accepts on a listener of its own, and the
/// kernel spreads connections over them. Its sockets are on an io_uring of
/// its own (`uring.rs`), or where that does not work, an epoll of its own
/// (`epoll.rs`).
#[cfg(target_os = "linux")]
fn run_linux<A: App>(
    main: &tokio::runtime::Runtime,
    listener: std::net::TcpListener,
    threads: usize,
) -> io::Result<()> {
    let mut rings = crate::uring::rings(threads).map(Vec::into_iter);
    // `listener` found the port free (and picked it, for port 0). The
    // workers' listeners share it, which its own would not allow.
    let addr = listener.local_addr()?;
    drop(listener);
    let listeners = (0..threads)
        .map(|_| crate::uring::listen(addr).map_err(|e| cannot_listen(addr, e)))
        .collect::<io::Result<Vec<_>>>()?;
    // Each worker says when its io_uring or epoll is set up; the server is
    // started once all are. (One that cannot be set up ends the process.)
    let (ready, all_ready) = std::sync::mpsc::channel();
    let workers = listeners
        .into_iter()
        .enumerate()
        .map(|(i, listener)| {
            let ring = rings.as_mut().and_then(Iterator::next);
            let ready = ready.clone();
            worker(i, move || async move {
                match ring {
                    Some(ring) => {
                        tokio::spawn(crate::uring::serve(ring, listener, ringed::<A>, ready))
                    }
                    None => tokio::spawn(crate::epoll::serve(
                        listener,
                        polled::<A>,
                        answers::<A>().then_some(on_driver::<A>),
                        ready,
                    )),
                };
                std::future::pending().await
            })
        })
        .collect::<io::Result<Vec<_>>>()?;
    drop(ready);
    for _ in 0..threads {
        // Only a worker that ended (and with it the process) sends nothing.
        let _ = all_ready.recv();
    }
    started(addr);
    main.block_on(async {
        stop_signal().await;
        stop(&workers).await; // the workers close their listeners
        Ok(())
    })
}

/// Worker `i`: a thread with a single-threaded tokio runtime of its own,
/// which runs `main` (it never ends) and what is spawned on the handle.
#[cfg(not(target_arch = "wasm32"))]
fn worker<F: Future<Output = ()>>(
    i: usize,
    main: impl FnOnce() -> F + Send + 'static,
) -> io::Result<tokio::runtime::Handle> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let handle = runtime.handle().clone();
    std::thread::Builder::new()
        .name(format!("wisp-{i}"))
        .spawn(move || runtime.block_on(main()))?;
    Ok(handle)
}

/// A connection a worker's io_uring accepted, served on that worker.
#[cfg(target_os = "linux")]
fn ringed<A: App>(stream: std::net::TcpStream) {
    let Some((slot, peer)) = admit(&stream) else {
        return;
    };
    let conn = Conn::Ring(crate::uring::Sock::new(stream));
    tokio::spawn(async move {
        connection::<A>(conn, peer).await;
        drop(slot);
    });
}

/// A connection a worker's epoll accepted, served on that worker: by its
/// driver while it can, else by its task (`epoll::spawn`).
#[cfg(target_os = "linux")]
fn polled<A: App>(stream: std::net::TcpStream) {
    let Some((slot, peer)) = admit(&stream) else {
        return;
    };
    crate::epoll::spawn(stream, peer, move |sock| async move {
        connection::<A>(Conn::Poll(sock), peer).await;
        drop(slot);
    });
}

/// A slot for a connection a worker accepted, and its peer; `None` when it
/// was refused at the cap, or is gone already.
#[cfg(target_os = "linux")]
fn admit(stream: &std::net::TcpStream) -> Option<(Slot, SocketAddr)> {
    let Some(slot) = Slot::take(crate::settings().max_conns) else {
        refuse(stream);
        return None;
    };
    Some((slot, stream.peer_addr().ok()?))
}

/// Stopping: new connections are already refused (Linux workers refuse
/// them as this begins). A connection answers what it is receiving or
/// working on with `connection: close` and closes, and the responses the
/// drivers still send go out; an idle one is closed as the process exits,
/// as nginx does, and a client retries on another connection. Waits at
/// most `DRAIN_TIMEOUT`, or until a second signal.
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
                    let _ = tx.send(BUSY.get() + sending() as isize);
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

thread_local! {
    /// Responses this thread's driver has yet to send (on Linux; elsewhere a
    /// connection sends its own before it is idle).
    static SENDING: Cell<usize> = const { Cell::new(0) };
}

/// [`SENDING`] on this thread.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn sending() -> usize {
    SENDING.get()
}

/// A send the driver finishes began (`true`) or ended.
#[cfg(target_os = "linux")]
pub(crate) fn send_under_way(began: bool) {
    SENDING.set(if began {
        SENDING.get() + 1
    } else {
        SENDING.get() - 1
    });
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
static CONNS: AtomicUsize = AtomicUsize::new(0);

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
fn refuse(mut s: &std::net::TcpStream) {
    let _ = s.write_all(b"HTTP/1.1 503 Service Unavailable\r\nretry-after: 1\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Slot {
    fn drop(&mut self) {
        CONNS.fetch_sub(1, Ordering::Relaxed);
    }
}

/// A connection counted against its worker in [`run_tokio`], until it drops.
#[cfg(not(any(target_arch = "wasm32", target_os = "linux")))]
struct Held(std::sync::Arc<[AtomicUsize]>, usize);

#[cfg(not(any(target_arch = "wasm32", target_os = "linux")))]
impl Drop for Held {
    fn drop(&mut self) {
        self.0[self.1].fetch_sub(1, Ordering::Relaxed);
    }
}

/// Binds, with what to do about the usual failures.
fn bind(addr: SocketAddr) -> io::Result<std::net::TcpListener> {
    std::net::TcpListener::bind(addr).map_err(|e| cannot_listen(addr, e))
}

/// [`bind`], trying again for a moment when the port is taken: a server killed
/// a moment ago keeps its io_uring listeners until the kernel has torn its
/// ring down, and a restart must not lose that race. A copy that still runs
/// holds the port past the wait.
#[cfg(not(target_arch = "wasm32"))]
fn bind_after_exit(addr: SocketAddr) -> io::Result<std::net::TcpListener> {
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        match bind(addr) {
            Err(e) if e.kind() == io::ErrorKind::AddrInUse && Instant::now() < until => {
                std::thread::sleep(Duration::from_millis(20));
            }
            r => return r,
        }
    }
}

fn cannot_listen(addr: SocketAddr, e: io::Error) -> io::Error {
    let hint = match e.kind() {
        io::ErrorKind::AddrInUse => {
            "Another program, maybe another copy of this one, is using the port. Stop it, or set PORT to use another."
        }
        io::ErrorKind::PermissionDenied => {
            "The port is reserved or needs more privileges (below 1024 on Linux). Set PORT to use another."
        }
        io::ErrorKind::AddrNotAvailable => {
            "This machine has no such address. Set HOST to one of its own, or to 0.0.0.0 for all."
        }
        _ => "",
    };
    io::Error::new(
        e.kind(),
        format!("cannot listen on {addr}: {e}\n  {hint}")
            .trim_end()
            .to_string(),
    )
}

/// A connection's socket: tokio's, or on Linux one on its worker's io_uring
/// or epoll.
#[cfg(not(target_arch = "wasm32"))]
enum Conn {
    Tcp(TcpStream),
    #[cfg(target_os = "linux")]
    Ring(crate::uring::Sock),
    #[cfg(target_os = "linux")]
    Poll(crate::epoll::Sock),
}

#[cfg(not(target_arch = "wasm32"))]
impl Conn {
    /// Reads what came onto the end of `buf`: `Ok(0)` when the peer closed.
    /// Safe to drop unfinished.
    async fn read(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        match self {
            Conn::Tcp(s) => s.read_buf(buf).await,
            #[cfg(target_os = "linux")]
            Conn::Ring(s) => s.read(buf).await,
            #[cfg(target_os = "linux")]
            Conn::Poll(s) => s.read(buf).await,
        }
    }

    /// Writes all of `buf` (see [`write`]) and leaves it empty. On the ring
    /// it is only queued, to go with the other connections' sends; on the
    /// epoll what the socket has no room for goes on without the caller. A
    /// failure then shows in the next call.
    async fn write(&mut self, buf: &mut Vec<u8>) -> io::Result<()> {
        match self {
            Conn::Tcp(s) => {
                write(s, buf).await?;
                buf.clear();
                Ok(())
            }
            #[cfg(target_os = "linux")]
            Conn::Ring(s) => s.write(buf).await,
            #[cfg(target_os = "linux")]
            Conn::Poll(s) => s.write(buf).await,
        }
    }

    /// What came onto the end of `buf`, without waiting: `None` when nothing
    /// has (a wakeup with nothing behind it), else as [`Conn::read`]. Only
    /// where `waits_bare`.
    fn try_read(&mut self, buf: &mut Vec<u8>) -> Option<io::Result<usize>> {
        let read = match self {
            Conn::Tcp(s) => s.try_read_buf(buf),
            #[cfg(target_os = "linux")]
            Conn::Ring(_) => return None,
            #[cfg(target_os = "linux")]
            Conn::Poll(s) => s.try_read(buf),
        };
        match read {
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => None,
            r => Some(r),
        }
    }

    /// It can wait for something to read without a buffer to read it into
    /// (`readable`). The ring cannot: it receives into the connection's
    /// buffer as it waits.
    fn waits_bare(&self) -> bool {
        match self {
            Conn::Tcp(_) => true,
            #[cfg(target_os = "linux")]
            Conn::Ring(_) => false,
            #[cfg(target_os = "linux")]
            Conn::Poll(_) => true,
        }
    }

    /// What the epoll driver received for this connection and left to it
    /// (see [`on_driver`]), once `readable`.
    fn handed(&mut self) -> Option<Handed> {
        match self {
            #[cfg(target_os = "linux")]
            Conn::Poll(s) => s.handed(),
            _ => None,
        }
    }

    /// Ends the sending side, once all of it is sent.
    async fn shutdown(&mut self) {
        match self {
            Conn::Tcp(s) => {
                let _ = s.shutdown().await;
            }
            #[cfg(target_os = "linux")]
            Conn::Ring(s) => s.shutdown().await,
            #[cfg(target_os = "linux")]
            Conn::Poll(s) => s.shutdown().await,
        }
    }

    /// The socket as tokio's, for a WebSocket, and what came after the
    /// handshake: `early`, then what the ring received that was not read.
    async fn into_tcp(self, early: Vec<u8>) -> io::Result<(TcpStream, Vec<u8>)> {
        match self {
            Conn::Tcp(s) => Ok((s, early)),
            #[cfg(target_os = "linux")]
            Conn::Ring(s) => s.into_tcp(early).await,
            #[cfg(target_os = "linux")]
            Conn::Poll(s) => s.into_tcp(early).await,
        }
    }
}

/// Reads more into `buf`: `Ok(0)` when the peer closed, `TimedOut` if
/// nothing came by `deadline` (in `seconds()`).
///
/// `timer` is the connection's one timer. Deadlines are whole seconds, so
/// it is moved at most once a second, and a later deadline only updates it
/// in place: a timer made and dropped per read cost two lock round trips on
/// the timer wheel and a clock read, every request. On the epoll the
/// driver keeps the deadline instead, and `timer` goes unused.
#[cfg(not(target_arch = "wasm32"))]
async fn read(
    stream: &mut Conn,
    buf: &mut Vec<u8>,
    timer: std::pin::Pin<&mut tokio::time::Sleep>,
    deadline: u64,
) -> io::Result<usize> {
    #[cfg(target_os = "linux")]
    if let Conn::Poll(s) = stream {
        return s.read_by(buf, deadline).await;
    }
    by(timer, deadline, stream.read(buf)).await
}

/// Waits until the socket has something to read (or its end, or an error,
/// which the read after it reports), failing with `TimedOut` if nothing
/// came by `deadline`, as [`read`] does. Only where `waits_bare`.
#[cfg(not(target_arch = "wasm32"))]
async fn readable(
    stream: &mut Conn,
    timer: std::pin::Pin<&mut tokio::time::Sleep>,
    deadline: u64,
) -> io::Result<()> {
    match stream {
        #[cfg(target_os = "linux")]
        Conn::Poll(s) => s.readable_by(deadline).await,
        #[cfg(target_os = "linux")]
        Conn::Ring(_) => Ok(()),
        Conn::Tcp(s) => by(timer, deadline, s.readable()).await,
    }
}

/// `f`, unless `deadline` (in `seconds()`) comes first: then `TimedOut`.
#[cfg(not(target_arch = "wasm32"))]
async fn by<T>(
    mut timer: std::pin::Pin<&mut tokio::time::Sleep>,
    deadline: u64,
    f: impl Future<Output = io::Result<T>>,
) -> io::Result<T> {
    let when = instant(deadline);
    if timer.deadline() != when {
        timer.as_mut().reset(when);
    }
    first(f, async {
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
                    if let Ok(s) = stream.into_std() {
                        refuse(&s);
                    }
                    continue;
                };
                let _ = stream.set_nodelay(true);
                tokio::spawn(async move {
                    connection::<A>(Conn::Tcp(stream), peer).await;
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
    if crate::settings().dev {
        dev::listed(A::ROOT, "/"); // lists `static/` now, not in the first request
    }
    HEAD_TAGS.get_or_init(|| {
        let mut s = String::new();
        if let Some(v) = A::CSS {
            s.push_str(&format!(
                "<link rel=\"stylesheet\" href=\"{}?v={v}\">",
                crate::protocol::APP_CSS_PATH
            ));
        }
        s.push_str(concat!(
            "<script defer src=\"",
            wisp_shared::app_path!("wisp.js"),
            "?v=",
            env!("WISP_RUNTIME_V"),
            "\"></script>"
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
pub(crate) fn accept_failed(e: &std::io::Error) -> bool {
    use std::io::ErrorKind::{ConnectionAborted, ConnectionReset, Interrupted};
    if matches!(e.kind(), ConnectionAborted | ConnectionReset | Interrupted) {
        return false;
    }
    // Once a second at most: out of descriptors, accepting is retried 20
    // times a second until some close.
    static LOGGED: AtomicU64 = AtomicU64::new(u64::MAX);
    if LOGGED.swap(seconds(), Ordering::Relaxed) != seconds() {
        log(format_args!("wisp: accept failed: {e}"));
    }
    true
}

/// A complete request, described by `cx`: its bytes, whether its client
/// asked to keep the connection open, and its route when a body's limit
/// had `parse` find it (its params are in `cx` then).
#[derive(Clone, Copy)]
struct Req {
    len: usize,
    keep_alive: bool,
    routed: Option<Option<usize>>,
}

enum Parsed {
    Request(Req),
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

/// What a connection works in: its `Cx` (and read buffer), write buffer,
/// page and reply, all reused across its requests. Boxed, so a connection
/// waiting holds a pointer at most, and handing them on moves only that.
struct Buffers {
    cx: Cx,
    wbuf: Vec<u8>,
    out: Out,
    reply: Reply,
}

/// How many `Buffers` a thread keeps for the requests to come.
const POOLED: usize = 64;

thread_local! {
    /// Buffers no request is using, last returned first: a connection waits
    /// for its next request without any, and takes the warmest set when it
    /// comes. So a thread whose requests are answered as they arrive works
    /// in one set, in cache, however many connections it serves, and a new
    /// connection allocates nothing. Boxed: a set moves as a pointer.
    #[allow(clippy::vec_box)]
    static POOL: std::cell::RefCell<Vec<Box<Buffers>>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Buffers for a request from `peer`.
#[cfg(not(target_arch = "wasm32"))]
fn take_buffers(peer: SocketAddr) -> Box<Buffers> {
    let mut b = POOL.with_borrow_mut(Vec::pop).unwrap_or_else(|| {
        Box::new(Buffers {
            cx: Cx::new(peer),
            wbuf: Vec::with_capacity(16 * 1024),
            out: Out::default(),
            reply: Reply::default(),
        })
    });
    b.cx.wire.peer = peer;
    b
}

/// `b` back, for the next request on this thread. Each request reset its
/// `Cx` as it was answered.
#[cfg(not(target_arch = "wasm32"))]
fn give_buffers(mut b: Box<Buffers>) {
    reset_buffers(&mut b);
    POOL.with_borrow_mut(|p| {
        if p.len() < POOLED {
            p.push(b);
        }
    });
}

/// `b` emptied of what came and what went, for its next request.
#[cfg(not(target_arch = "wasm32"))]
fn reset_buffers(b: &mut Buffers) {
    b.cx.wire.buf.clear();
    b.wbuf.clear();
    trim_buffers(b);
}

/// What one large request or response grew `b` to goes; the read buffer
/// only once it is empty.
#[cfg(not(target_arch = "wasm32"))]
fn trim_buffers(b: &mut Buffers) {
    if b.cx.wire.buf.is_empty() {
        policy::trim(&mut b.cx.wire.buf, READ_CAPACITY);
    }
    policy::trim(&mut b.wbuf, KEEP_CAPACITY);
    if !policy::kept(b.out.body.capacity()) {
        b.out = Out::default();
    }
    if !policy::kept_headers(b.reply.headers.capacity()) {
        b.reply.headers = Vec::new();
    }
}

/// What the epoll driver received for a connection and leaves to its
/// future (see [`on_driver`]): the buffers, how much of `cx.wire.buf` is
/// answered, and the request after that when the driver got that far with
/// it (see [`Ahead`]).
pub(crate) struct Handed {
    held: Holding,
    at: usize,
    ahead: Option<Ahead>,
}

/// The buffers of what is handed, or the request deciding in them: one the
/// build said never waits, which did. The connection's future goes on
/// polling it, so what it did so far is never lost.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
enum Holding {
    Ready(Box<Buffers>),
    Deciding(Deciding),
}

/// A decider (see [`decider`]) with a request under way.
type Deciding = std::pin::Pin<Box<dyn Future<Output = ()> + Send>>;

impl Handed {
    #[cfg(target_os = "linux")]
    fn new(held: Holding, at: usize, ahead: Option<Ahead>) -> Handed {
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
type Ahead = (Req, bool);

/// Whether some route of `A` answers without waiting, so [`on_driver`] can help.
#[cfg(target_os = "linux")]
fn answers<A: App>() -> bool {
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
fn park(mut b: Box<Buffers>) {
    reset_buffers(&mut b);
    DRIVER.set(Some(b));
}

/// [`on_driver`]'s requests in `b.cx.wire.buf`, answered into `b.wbuf`:
/// the buffers when that was all of them, else what is left for the
/// connection's future.
#[cfg(target_os = "linux")]
fn answer_whole<A: App>(mut b: Box<Buffers>) -> Result<Box<Buffers>, Handed> {
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
        serialize::<A>(
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
async fn decider<A: App>() {
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
                    tag(cx, reply);
                }
            }
        }
        OUTBOX.set(Some(b));
    }
}

/// What the decider is to do for a request.
#[cfg(target_os = "linux")]
enum Job {
    /// All of [`decide`], routed to this.
    Decide(Option<usize>),
    /// The error page of what [`decide_now`] decided.
    Page(Failed),
}

/// A request [`decide_now`] decided but for its error page, which is
/// rendered in a future: an error page may wait.
#[cfg(target_os = "linux")]
struct Failed {
    route: Option<usize>,
    page: (u16, Cow<'static, str>),
    failure: Option<String>,
    started: Option<Instant>,
}

/// [`decide`], for an arm of the request's route that [`App::handle_now`]
/// answers: with no future, unless for an error page. What is left for the
/// decider, if anything: all of it when the arm turns out to have no sync
/// form, as nothing was done that `decide` does not do again.
#[cfg(target_os = "linux")]
fn decide_now<A: App>(
    cx: &mut Cx,
    out: &mut Out,
    reply: &mut Reply,
    route: Option<usize>,
) -> Option<Job> {
    if crate::settings().request_id {
        cx.request_id();
    }
    if !before_routes::<A>(cx, route, reply) {
        let started = timed().then(Instant::now);
        out.clear();
        let Some(result) = catch_now(|| A::handle_now(route, cx, out)) else {
            return Some(Job::Decide(route));
        };
        let (failure, page) = settle(cx, out, reply, result);
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
    tag(cx, reply);
    None
}

/// Decides the request in `b`, routed to `route`, on this thread's
/// decider: its buffers when that is done at once, else the decider, still
/// deciding it (see [`decided`]).
#[cfg(target_os = "linux")]
fn at_once<A: App>(b: Box<Buffers>, job: Job) -> Result<Box<Buffers>, Deciding> {
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
async fn decided(mut d: Deciding) -> Box<Buffers> {
    std::future::poll_fn(|cx| {
        let _ = d.as_mut().poll(cx);
        OUTBOX.take().map_or(Poll::Pending, Poll::Ready)
    })
    .await
}

#[cfg(not(target_arch = "wasm32"))]
async fn connection<A: App>(stream: Conn, peer: SocketAddr) {
    let mut held = Some(take_buffers(peer));
    requests::<A>(stream, peer, &mut held).await;
    if let Some(b) = held {
        give_buffers(b);
    }
}

/// Answers the requests of one connection, until it closes.
#[cfg(not(target_arch = "wasm32"))]
async fn requests<A: App>(mut stream: Conn, peer: SocketAddr, held: &mut Option<Box<Buffers>>) {
    let mut busy = Busy(false);
    let mut head_since: Option<u64> = None;
    let mut body_since: Option<u64> = None;
    let mut sent_continue = false;
    let mut timer = std::pin::pin!(tokio::time::sleep_until(instant(0)));
    // Where the driver stopped in what it handed over, and the request
    // there when it got that far (see `Handed`).
    let mut start = 0;
    let mut ahead: Option<Ahead> = None;

    'conn: loop {
        let b = &mut **held.get_or_insert_with(|| take_buffers(peer));
        let mut used = std::mem::take(&mut start);
        let mut need = 0;
        let mut in_body = false;
        let mut close = false;
        let mut refused = false;
        while used < b.cx.wire.buf.len() {
            let (parsed, decided) = match ahead.take() {
                Some((req, decided)) => (Parsed::Request(req), decided),
                None => (parse::<A>(&mut b.cx, used, true), false),
            };
            let Buffers {
                cx,
                wbuf,
                out,
                reply,
            } = &mut *b;
            match parsed {
                Parsed::Request(req) => {
                    let keep_alive = policy::keeps_open(req.keep_alive, stopping());
                    if !decided {
                        decide::<A>(cx, out, reply, req.routed).await;
                    }
                    // Only as a 101: a hook may have answered otherwise.
                    let upgrade = reply.take_websocket().filter(|_| reply.status == 101);
                    let streamed = serialize::<A>(
                        wbuf,
                        reply,
                        out,
                        cx.wire.http11,
                        keep_alive || upgrade.is_some(),
                        cx.method == Method::Head,
                    );
                    cx.reset();
                    used += req.len;
                    // Answers to many small pipelined requests go out in
                    // pieces, so a buffer of them cannot make a huge one.
                    if streamed.is_none()
                        && upgrade.is_none()
                        && wbuf.len() >= KEEP_CAPACITY
                        && stream.write(wbuf).await.is_err()
                    {
                        return;
                    }
                    if let Some(upgrade) = upgrade {
                        // The rest of the connection is the WebSocket's,
                        // on tokio's socket, with what the client sent
                        // after its handshake. It holds none of the
                        // buffers meanwhile.
                        let early = cx.wire.buf[used..].to_vec();
                        let (limit, path) = (body_limit::<A>(cx.path()), cx.path().to_owned());
                        let sent = stream.write(wbuf).await.is_ok();
                        give_buffers(held.take().expect("taken at the top of the loop"));
                        if sent && let Ok((tcp, early)) = stream.into_tcp(early).await {
                            crate::ws::serve(tcp, early, limit, upgrade, &path).await;
                        }
                        return;
                    }
                    head_since = None;
                    body_since = None;
                    sent_continue = false;
                    if let Some(s) = streamed {
                        // What was answered so far goes first, then the body
                        // as it comes, which may take hours (server-sent
                        // events): meanwhile the connection holds none of
                        // the buffers, and what the client sends (a request
                        // after this one) waits in `early`.
                        if stream.write(wbuf).await.is_err() {
                            return;
                        }
                        let mut early = cx.wire.buf[used..].to_vec();
                        give_buffers(held.take().expect("taken at the top of the loop"));
                        let mut w = Vec::new();
                        if pump(&mut stream, s.body, s.chunked, &mut w, &mut early)
                            .await
                            .is_err()
                        {
                            return;
                        }
                        if s.close {
                            stream.shutdown().await;
                            return;
                        }
                        held.insert(take_buffers(peer))
                            .cx
                            .wire
                            .buf
                            .extend_from_slice(&early);
                        continue 'conn;
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
                    if policy::continues(expect_continue, sent_continue) {
                        wbuf.extend_from_slice(b"HTTP/1.1 100 Continue\r\n\r\n");
                        sent_continue = true;
                    }
                    break;
                }
                Parsed::Invalid(status) => {
                    reply.set_plain(status, reason(status));
                    serialize::<A>(wbuf, reply, out, true, false, false);
                    (close, refused) = (true, true);
                    break;
                }
            }
        }

        if !b.wbuf.is_empty() && stream.write(&mut b.wbuf).await.is_err() {
            return;
        }
        if close {
            stream.shutdown().await;
            if refused {
                linger(&mut stream, &mut b.cx.wire.buf, timer.as_mut()).await;
            }
            return;
        }

        b.cx.wire.buf.drain(..used);
        trim_buffers(b);
        let have = b.cx.wire.buf.len();
        b.cx.wire.buf.reserve(policy::read_ahead(need, have));

        let deadline = if have == 0 {
            busy.set(false);
            if stopping() {
                return;
            }
            policy::idle_deadline(seconds())
        } else if in_body {
            let since = *body_since.get_or_insert_with(seconds);
            policy::body_deadline(seconds(), since, have)
        } else {
            policy::head_deadline(*head_since.get_or_insert_with(seconds))
        };
        if have == 0 && stream.waits_bare() {
            // Nothing of a next request yet: wait for it without buffers,
            // which go to whichever request on this thread comes first.
            give_buffers(held.take().expect("taken at the top of the loop"));
            loop {
                if readable(&mut stream, timer.as_mut(), deadline)
                    .await
                    .is_err()
                {
                    return; // closed, error or too slow
                }
                if let Some(h) = stream.handed() {
                    busy.set(true);
                    *held = Some(match h.held {
                        Holding::Ready(b) => b,
                        Holding::Deciding(d) => decided(d).await,
                    });
                    (start, ahead) = (h.at, h.ahead);
                    continue 'conn;
                }
                let b = held.insert(take_buffers(peer));
                match stream.try_read(&mut b.cx.wire.buf) {
                    Some(Ok(n)) if n > 0 => break,
                    Some(_) => return,
                    // Readable with nothing to read (a new socket, which
                    // the epoll tries at once; tokio's on Windows, which
                    // says so of every new one): it waits again holding
                    // nothing, rather than in a read holding the buffers.
                    None => give_buffers(held.take().expect("just taken")),
                }
            }
            busy.set(true);
            continue;
        }
        match read(&mut stream, &mut b.cx.wire.buf, timer.as_mut(), deadline).await {
            Ok(n) if n > 0 => busy.set(true),
            _ => return, // closed, error or too slow
        }
    }
}

/// After a refused request: what the client still sends (the rest of a body
/// too large, say) is read and dropped for a moment, up to a limit. A socket
/// closed with bytes unread resets the connection, and the client may lose
/// the answer before it has read it.
#[cfg(not(target_arch = "wasm32"))]
async fn linger(
    stream: &mut Conn,
    buf: &mut Vec<u8>,
    mut timer: std::pin::Pin<&mut tokio::time::Sleep>,
) {
    let deadline = seconds() + policy::LINGER.as_secs();
    let mut left = policy::LINGER_BYTES;
    while left > 0 {
        buf.clear();
        match read(stream, buf, timer.as_mut(), deadline).await {
            Ok(n) if n > 0 => left = left.saturating_sub(n),
            _ => return,
        }
    }
}

/// Parses the request starting at `cx.wire.buf[at..]` and, if it is complete,
/// records it in `cx` as spans. `on_wire`: from a client of the built-in
/// server, where HTTP/1.1 must name its host (RFC 9112 §3.2); a request
/// from another host (`Cx::from_request`) may have none.
fn parse<A: App>(cx: &mut Cx, at: usize, on_wire: bool) -> Parsed {
    cx.wire.headers.clear();
    let head = match fast_head(&cx.wire.buf, at, &mut cx.wire.headers) {
        Some(head) => head,
        None => {
            cx.wire.headers.clear();
            match slow_head(&cx.wire.buf, at, &mut cx.wire.headers) {
                Ok(head) => head,
                Err(parsed) => return parsed,
            }
        }
    };
    let buf = &cx.wire.buf[..];

    let mut content_length: Option<usize> = None;
    let mut transfer_encodings = 0;
    let mut hosts = 0;
    let mut keep_alive = head.http11;
    let mut expect_continue = false;
    cx.wire.knows = 0;
    for &(name, span) in &cx.wire.headers {
        // Only these five matter here, and their lengths tell them apart:
        // one comparison a header at most.
        let is = |lower: &[u8]| swar::eq_lower(&buf[name.range()], lower);
        let value = || &buf[span.range()];
        match name.len {
            4 if is(b"host") => hosts += 1,
            14 if is(b"content-length") => {
                // Strict: digits only, and repeated headers must agree (smuggling).
                match (parse_decimal(value()), content_length) {
                    (Some(n), None) => content_length = Some(n),
                    (Some(n), Some(m)) if n == m => {}
                    _ => return Parsed::Invalid(400),
                }
            }
            17 if is(b"transfer-encoding") => {
                // Only `chunked`, alone: gzip and the like are for a proxy.
                if !swar::eq_lower(value().trim_ascii(), b"chunked") {
                    return Parsed::Invalid(501);
                }
                transfer_encodings += 1;
            }
            // One token, the usual case, before splitting the list.
            10 if is(b"connection") => match value().trim_ascii() {
                v if swar::eq_lower(v, b"keep-alive") => keep_alive = true,
                v if swar::eq_lower(v, b"close") => keep_alive = false,
                v => {
                    for token in v.split(|&b| b == b',').map(<[u8]>::trim_ascii) {
                        if swar::eq_lower(token, b"close") {
                            keep_alive = false;
                        } else if swar::eq_lower(token, b"keep-alive") {
                            keep_alive = true;
                        }
                    }
                }
            },
            // Never to HTTP/1.0, which has no such answer (RFC 9110 §10.1.1).
            6 if is(b"expect") => {
                expect_continue =
                    head.http11 && swar::eq_lower(value().trim_ascii(), b"100-continue");
            }
            _ => {
                if let Some(k) = Known::of(&buf[name.range()])
                    && cx.wire.knows & 1 << k == 0
                {
                    cx.wire.knows |= 1 << k;
                    cx.wire.known[k] = span;
                }
            }
        }
    }
    let chunked = transfer_encodings > 0;
    // Both framings at once is the classic smuggling vector; HTTP/1.0 has
    // no chunked framing at all. Two hosts could be read as either, by
    // Wisp and a proxy or cache in front of it (RFC 9112 §3.2).
    if (chunked && (content_length.is_some() || transfer_encodings > 1 || !head.http11))
        || hosts > 1
    {
        return Parsed::Invalid(400);
    }

    cx.method = head.method;
    cx.wire.http11 = head.http11;
    cx.wire.path = head.path;
    cx.wire.query = head.query;
    // A body's limit is its route's: the request is routed for it now, once.
    let body_start = at + head.len;
    let len = content_length.unwrap_or(0);
    let mut routed = None;
    let mut limit = 0;
    if chunked || len > 0 {
        let route = cx.path().starts_with('/').then(|| route::<A>(cx));
        limit = limit_of::<A>(route.flatten());
        routed = route;
    }
    let buf = &cx.wire.buf[..];
    let (len, total) = if chunked {
        match chunks(&buf[body_start..], limit) {
            Chunks::Complete { wire, body } => (body, head.len + wire),
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
        if len > limit {
            return Parsed::Invalid(413);
        }
        if buf.len() - body_start < len {
            return Parsed::Partial {
                need: head.len + len,
                expect_continue,
                body: true,
            };
        }
        (len, head.len + len)
    };

    // Once whole, as the limits come first.
    if on_wire && hosts == 0 && head.http11 {
        return Parsed::Invalid(400);
    }
    cx.wire.body = Span {
        start: body_start as u32,
        len: len as u32,
    };
    if chunked {
        // The body's data moves up over the chunk framing, into one piece
        // where `cx.wire.body` says. The head stays as it is.
        unchunk(&mut cx.wire.buf[body_start..at + total]);
    }
    Parsed::Request(Req {
        len: total,
        keep_alive,
        routed,
    })
}

/// A request's head: its line, and its headers, which went to `cx.wire.headers`.
struct Head {
    /// Its bytes, up to the body.
    len: usize,
    method: Method,
    path: Span,
    query: Span,
    /// HTTP/1.1 rather than 1.0.
    http11: bool,
}

/// The bytes of a header name (RFC 9110 `tchar`), as httparse takes them.
const TOKEN: [bool; 256] = {
    let mut t = [false; 256];
    let mut c = 0;
    while c < 256 {
        t[c] = (c as u8).is_ascii_alphanumeric();
        c += 1;
    }
    let symbols = b"!#$%&'*+-.^_`|~";
    let mut k = 0;
    while k < symbols.len() {
        t[symbols[k] as usize] = true;
        k += 1;
    }
    t
};

/// The head of `buf[at..]` when it has the usual shape: a method in
/// capitals, a target of visible ASCII, `HTTP/1.1` or `HTTP/1.0`, lines that
/// end in CRLF, header names of `tchar`s, and all of it here, in at most
/// `MAX_HEAD` bytes. Header values are scanned 16 bytes at a time. It is
/// `None` for anything else, which [`slow_head`] (httparse) then reads,
/// or refuses: what this reads, httparse reads the same.
fn fast_head(buf: &[u8], at: usize, headers: &mut Vec<(Span, Span)>) -> Option<Head> {
    let b = &buf[..buf.len().min(at + MAX_HEAD)];
    let mut i = at;
    let method = if b.get(i..i + 4) == Some(b"GET ") {
        i += 3;
        Method::Get
    } else {
        while b.get(i).is_some_and(u8::is_ascii_uppercase) {
            i += 1;
        }
        Method::parse(&b[at..i])
    };
    if i == at || b.get(i) != Some(&b' ') {
        return None;
    }
    i += 1;

    // The target, to the first byte that is not visible ASCII: its space.
    let target = i;
    while i + 8 <= b.len() {
        let x = swar::word(b, i);
        let stop = swar::below(x, 0x21) | swar::above(x, 0x7e);
        if stop != 0 {
            i += swar::first(stop);
            break;
        }
        i += 8;
    }
    while b.get(i).is_some_and(|c| (0x21..=0x7e).contains(c)) {
        i += 1;
    }
    let end = i;
    let http11 = match b.get(i..i + 11)? {
        b" HTTP/1.1\r\n" => true,
        b" HTTP/1.0\r\n" => false,
        _ => return None,
    };
    // Empty (its space), or not a path: `*`, `http://host/x`.
    if b[target] != b'/' {
        return None;
    }
    i += 11;
    let span = |from: usize, to: usize| Span {
        start: from as u32,
        len: (to - from) as u32,
    };
    let (path, query) = match b[target..end].iter().position(|&c| c == b'?') {
        Some(q) => (span(target, target + q), span(target + q + 1, end)),
        None => (span(target, end), Span::default()),
    };

    loop {
        if b.get(i..i + 2)? == b"\r\n" {
            return Some(Head {
                len: i + 2 - at,
                method,
                path,
                query,
                http11,
            });
        }
        // The name: letters, digits and `-` eight at a time, any other
        // `tchar` one at a time.
        let name = i;
        loop {
            if i + 8 <= b.len() {
                let miss = swar::not_name(swar::word(b, i));
                if miss == 0 {
                    i += 8;
                    continue;
                }
                i += swar::first(miss);
            }
            if !b.get(i).is_some_and(|&c| TOKEN[usize::from(c)]) {
                break;
            }
            i += 1;
        }
        if i == name || b.get(i) != Some(&b':') {
            return None;
        }
        let name = span(name, i);
        i += 1;
        while b.get(i).is_some_and(|&c| c == b' ' || c == b'\t') {
            i += 1;
        }
        // The value, to its CR: past visible ASCII, spaces, tabs and
        // bytes of 0x80 and up; any other control stops it.
        let value = i;
        loop {
            if i + 8 <= b.len() {
                let stop = swar::control(swar::word(b, i));
                if stop == 0 {
                    i += 8;
                    // Past eight, a long value (a cookie, a user agent):
                    // sixteen at a time.
                    while let Some(chunk) = b.get(i..i + 16)
                        && !swar::any_control(chunk)
                    {
                        i += 16;
                    }
                    continue;
                }
                i += swar::first(stop);
            } else {
                while b.get(i).is_some_and(|&c| !swar::is_control(c)) {
                    i += 1;
                }
            }
            if b.get(i) != Some(&b'\t') {
                break;
            }
            i += 1;
        }
        if b.get(i..i + 2)? != b"\r\n" {
            return None;
        }
        let mut value_end = i;
        while value_end > value && matches!(b[value_end - 1], b' ' | b'\t') {
            value_end -= 1;
        }
        if headers.len() == MAX_HEADERS {
            return None;
        }
        headers.push((name, span(value, value_end)));
        i += 2;
    }
}

/// The head of `buf[at..]` by httparse, which takes what [`fast_head`] does
/// not: bare LF line ends, empty lines before the request, other methods
/// and targets. `Err` is what to answer: wait for more, or refuse it.
fn slow_head(buf: &[u8], at: usize, headers: &mut Vec<(Span, Span)>) -> Result<Head, Parsed> {
    // Left uninitialized: zeroing them cost more than parsing a small request.
    let mut raw = [const { MaybeUninit::uninit() }; MAX_HEADERS];
    let mut req = httparse::Request::new(&mut []);
    let len = match req.parse_with_uninit_headers(&buf[at..], &mut raw) {
        Ok(httparse::Status::Complete(n)) if n <= MAX_HEAD => n,
        Ok(httparse::Status::Partial) if buf.len() - at <= MAX_HEAD => {
            return Err(Parsed::Partial {
                need: 0,
                expect_continue: false,
                body: false,
            });
        }
        Ok(_) | Err(httparse::Error::TooManyHeaders) => return Err(Parsed::Invalid(431)),
        Err(_) => return Err(Parsed::Invalid(400)),
    };
    let target = req.path.unwrap_or("").as_bytes();
    let (mut path, query) = match target.iter().position(|&c| c == b'?') {
        Some(q) => (&target[..q], &target[q + 1..]),
        None => (target, &b""[..]),
    };
    // The absolute form a proxy sends, `http://host/x` (RFC 9112 §3.2.2): its
    // path, and its host in place of the `host` header's.
    let mut host = None;
    if let Some((authority, origin)) = absolute_form(path) {
        (host, path) = (Some(Span::of(buf, authority)), origin);
    }
    for h in req.headers.iter() {
        let value = match host {
            Some(host) if h.name.eq_ignore_ascii_case("host") => host,
            _ => Span::of(buf, h.value),
        };
        headers.push((Span::of(buf, h.name.as_bytes()), value));
    }
    Ok(Head {
        len,
        method: Method::parse(req.method.unwrap_or("").as_bytes()),
        path: Span::of(buf, path),
        query: Span::of(buf, query),
        http11: req.version == Some(1),
    })
}

/// The authority and path of an absolute-form target's path part
/// (`http://host:80/x`): `http` or `https`, a host without user info. The
/// path of `http://host` is `/`, the slash before `host`.
fn absolute_form(t: &[u8]) -> Option<(&[u8], &[u8])> {
    if t.first() == Some(&b'/') {
        return None; // the usual origin form
    }
    let colon = t.iter().position(|&c| c == b':')?;
    let scheme = &t[..colon];
    if !(scheme.eq_ignore_ascii_case(b"http") || scheme.eq_ignore_ascii_case(b"https"))
        || t.get(colon + 1..colon + 3) != Some(b"//")
    {
        return None;
    }
    let host = colon + 3;
    let end = t[host..]
        .iter()
        .position(|&c| c == b'/')
        .map_or(t.len(), |i| host + i);
    if end == host || t[host..end].contains(&b'@') {
        return None;
    }
    let path = if end < t.len() {
        &t[end..]
    } else {
        &t[host - 1..host]
    };
    Some((&t[host..end], path))
}

/// The largest body any route takes, whatever its limit says: requests are
/// described by 32-bit offsets into the read buffer.
const MAX_BODY: usize = 1 << 31;

/// The body limit for a request to `path`: its route's `BODY_LIMIT`, or
/// `WISP_BODY_LIMIT`, and at most `MAX_BODY`. Only requests with a body
/// look it up.
pub(crate) fn body_limit<A: App>(path: &str) -> usize {
    let route = path.starts_with('/').then(|| A::route(path)).flatten();
    limit_of::<A>(route.map(|(id, _)| id))
}

/// [`body_limit`] of a request routed to `route`.
fn limit_of<A: App>(route: Option<usize>) -> usize {
    let usual = crate::settings().body_limit;
    route
        .and_then(|id| A::ROUTES[id].limit(usual))
        .unwrap_or(usual)
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
        let Ok(size) = usize::try_from(parse_hex(&b[i..i + digits])) else {
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
        let size = parse_hex(&b[i..i + digits]) as usize;
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
fn parse_hex(digits: &[u8]) -> u64 {
    digits
        .iter()
        .fold(0, |n, &c| n << 4 | u64::from(hex_digit(c).unwrap_or(0)))
}

thread_local! {
    /// The body of the last response this thread wrote, emptied, for the
    /// next one to fill: a warm server allocates no bodies.
    static SPARE: Cell<Vec<u8>> = const { Cell::new(Vec::new()) };
}

/// An empty buffer for a response body, with the room of an earlier one.
pub(crate) fn spare() -> Vec<u8> {
    SPARE.take()
}

/// [`spare`] for `len` bytes that may be kept long (a `String` input a
/// handler stores): only one about that size, else a new buffer, so a
/// small value never holds a large one's room.
pub(crate) fn spare_for(len: usize) -> Vec<u8> {
    let b = SPARE.take();
    if (len..=len.max(16) * 4).contains(&b.capacity()) {
        return b;
    }
    SPARE.set(b);
    Vec::with_capacity(len)
}

/// Keeps `body`, now written, for [`spare`] to hand out again, unless one
/// large message made it big.
fn recycle(mut body: Vec<u8>) {
    if policy::kept(body.capacity()) {
        body.clear();
        SPARE.set(body);
    }
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

    fn add(&mut self, headers: Vec<(Cow<'static, str>, String)>) {
        self.headers
            .extend(headers.into_iter().map(|(n, v)| (n, Cow::Owned(v))));
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

/// [`decide`] for a request of its own, with the page rendered and the
/// framing as [`serialize`] has it on the wire.
pub(crate) async fn answer<A: App>(mut cx: Cx) -> Reply {
    setup::<A>();
    let (mut out, mut reply) = (Out::default(), Reply::default());
    decide::<A>(&mut cx, &mut out, &mut reply, None).await;
    match reply.body {
        Body::WebSocket(_) => reply.set_plain(501, "WebSockets need Wisp's own server"),
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
    // HEAD gets the headers of a GET and no body; 204 and 304 have none.
    if head || bodiless {
        let len = (!matches!(reply.body, Body::Stream(_))).then(|| reply.bytes().len());
        reply.body = Body::Static(b"");
        if let Some(len) = len.filter(|_| reply.header("content-length").is_none() && !bodiless) {
            reply
                .headers
                .push((Cow::Borrowed("content-length"), Cow::Owned(len.to_string())));
        }
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
async fn decide<A: App>(
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
    if !before_routes::<A>(cx, route, reply) {
        let started = timed().then(Instant::now);
        out.clear();
        let result = catch_made(|| A::handle(route, cx, out)).await;
        let (failure, page) = settle(cx, out, reply, result);
        if let Some(page) = page {
            render_error::<A>(route, cx, out, page).await;
        }
        answered(cx, reply, started, failure);
    }
    tag(cx, reply);
}

/// The reply to what the handler did, `result` ([`answer_of`]), or to its
/// error ([`error_reply`]): what went wrong, for the log, and the error
/// page to render, if any.
#[allow(clippy::type_complexity)]
fn settle(
    cx: &mut Cx,
    out: &mut Out,
    reply: &mut Reply,
    result: crate::Result<()>,
) -> (Option<String>, Option<(u16, Cow<'static, str>)>) {
    match answer_of(cx, out, reply, result) {
        Ok(()) => (None, None),
        Err(e) => error_reply(cx, out, reply, e),
    }
}

/// The reply's `x-request-id`, when the request has an id.
fn tag(cx: &Cx, reply: &mut Reply) {
    if let Some(id) = cx.id() {
        reply
            .headers
            .push((Cow::Borrowed("x-request-id"), Cow::Owned(id.to_string())));
    }
}

/// What is answered before the routes: a path that is not one, Wisp's own
/// files, a trailing slash, the app's files. Whether it was.
fn before_routes<A: App>(cx: &Cx, route: Option<usize>, reply: &mut Reply) -> bool {
    // Its bytes: only a few of these need it as a `str`.
    let raw = cx.raw_path();
    if raw.first() != Some(&b'/') {
        reply.set_plain(400, "Bad Request");
        return true;
    }
    if raw.starts_with(b"/_") && internal::<A>(cx, cx.path(), reply) {
        return true;
    }
    if raw.len() > 1 && raw.ends_with(b"/") {
        // One leading slash: `//evil.example/` would send the browser to
        // another site (and so would `/\evil.example/`).
        let trimmed = cx.path().trim_matches(['/', '\\']);
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
        return true;
    }
    matches!(cx.method, Method::Get | Method::Head) && file::<A>(cx, raw, route, reply)
}

/// The reply to what the handler did, `result`: its response, page or
/// redirect; or the error an error page answers.
fn answer_of(
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
                Some(mut res) => {
                    if not_modified(cx, &res) {
                        res.status = 304;
                    }
                    reply.status = res.status;
                    reply.headers.clear();
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
                None => match out.made.take() {
                    Some(made) => crate::bake::reply(cx, made, reply),
                    None => reply.set(cx.status(), "text/html; charset=utf-8", Body::Page),
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

/// The reply decided: kept for its `Idempotency-Key`, given the headers the
/// request set, and logged; `failure` is what went wrong in a 5xx, for the
/// log (the page may say less).
fn answered(cx: &mut Cx, reply: &mut Reply, started: Option<Instant>, failure: Option<String>) {
    if let Some(key) = cx.idem.take() {
        key.finish(reply, cx.page_headers());
    }
    cx.send_headers(&mut reply.headers);
    let method = cx.method.as_str();
    if let Some(started) = started {
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

/// The route of the request in `cx`, which gets its params: the matched
/// parameters as spans, so `cx` can be handed out mutably.
fn route<A: App>(cx: &mut Cx) -> Option<usize> {
    let (id, raw) = A::route(cx.path())?;
    // Only the route's own: most have none.
    let names = A::ROUTES[id].params;
    if names.is_empty() {
        cx.clear_params();
        return Some(id);
    }
    let mut params = [Span::default(); crate::cx::MAX_PARAMS];
    for (p, s) in params.iter_mut().zip(raw).take(names.len()) {
        *p = Span::of(&cx.wire.buf, s.as_bytes());
    }
    cx.set_params(names, params);
    Some(id)
}

/// Whether `res`, a 200 to a GET or HEAD with an `etag` (an [`crate::Image`],
/// say), is what the client says it has: then it is a 304, with no body.
/// The response's own headers are few; the request's are looked through
/// only for one with an `etag`.
fn not_modified(cx: &Cx, res: &crate::Response) -> bool {
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
fn redirect_reply(cx: &Cx, e: Error, reply: &mut Reply) {
    let js = cx.header(crate::protocol::HEADER_JS).is_some();
    reply.set_plain(if js { 200 } else { e.status }, "");
    if let Some((name, value)) = e.header.map(|h| *h) {
        let name = if js && name == "location" {
            crate::protocol::HEADER_LOCATION
        } else {
            name
        };
        reply.headers.push((Cow::Borrowed(name), Cow::Owned(value)));
    }
}

/// The error page of `status` with `message`: the nearest `+error.wisp`,
/// else Wisp's own.
async fn render_error<A: App>(
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
fn error_reply(
    cx: &mut Cx,
    out: &mut Out,
    reply: &mut Reply,
    mut e: Error,
) -> (Option<String>, Option<(u16, Cow<'static, str>)>) {
    let failure = (e.status >= 500).then(|| e.detail());
    // 5xx details can leak internals; only dev shows them. An error that
    // says no more than its status's name gets a sentence about the status
    // instead.
    let message = if (e.status >= 500 && !crate::settings().dev)
        || e.message.is_empty()
        || e.message.eq_ignore_ascii_case(reason(e.status))
    {
        Cow::Borrowed(sentence(e.status))
    } else {
        std::mem::take(&mut e.message)
    };
    out.clear();
    // The headers of the page that failed go with it; the `before` hook's
    // stay.
    cx.drop_page_headers();
    let page = if wants_json(cx) {
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
        reply.set(e.status, "text/html; charset=utf-8", Body::Page);
        Some((e.status, message))
    };
    if let Some((name, value)) = e.header.take().map(|h| *h) {
        reply.headers.push((Cow::Borrowed(name), Cow::Owned(value)));
    }
    (failure, page)
}

/// Whether an error goes back as JSON rather than an error page: a request
/// under `/api`, one that sent JSON, one that asks for JSON and not HTML,
/// or one to a `+server.rs` endpoint from anything but a browser page.
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

/// Whether `name` is framing, which the host writes itself: an app's own
/// `content-length` or `transfer-encoding` (copied from another server's
/// response, say) would contradict it, and the client would read the next
/// response wrong. Only an answer to HEAD, which has no body to count, may
/// give its length.
fn framing(name: &str, head: bool) -> bool {
    match name.len() {
        14 => !head && name.eq_ignore_ascii_case("content-length"),
        17 => name.eq_ignore_ascii_case("transfer-encoding"),
        _ => false,
    }
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
    out: &mut Out,
    http11: bool,
    keep_alive: bool,
    head_only: bool,
) -> Option<Streamed> {
    let bodiless = bodiless(reply.status);
    let stream = matches!(reply.body, Body::Stream(_)) && !bodiless;
    // HTTP/1.0 has no chunks: a streamed body ends with the connection.
    let chunked = stream && http11;
    let keep_alive = keep_alive && (!stream || chunked || head_only);
    let parts = matches!(reply.body, Body::Page).then(|| page::<A>(out));
    let len = match &parts {
        Some(parts) => parts.iter().map(|p| p.len()).sum(),
        None => reply.bytes().len(),
    };

    // See `framing`.
    let own_length = head_only && reply.header("content-length").is_some();
    let made = matches!(reply.body, Body::Made(_));
    if let Body::Made(m) = &reply.body {
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
    let length = !made && !chunked && !stream && !bodiless && !own_length;
    length_and_date(w, length.then_some(len));
    if !keep_alive {
        w.extend_from_slice(b"connection: close\r\n");
    } else if !http11 {
        // HTTP/1.0 closes after each response unless told otherwise.
        w.extend_from_slice(b"connection: keep-alive\r\n");
    }
    for h in &reply.headers {
        if !framing(&h.0, head_only) {
            header(w, h);
        }
    }
    w.extend_from_slice(b"\r\n");

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

/// The parts of a page, in order: the shell around the tags for
/// `%wisp.head%`, the page's head, and its body, which its browser code
/// ends. Once a page.
pub(crate) fn page<A: App>(out: &mut Out) -> [&str; 6] {
    out.live.tail(&mut out.body);
    let [s0, s1, s2] = A::shell();
    let tags = HEAD_TAGS.get().map_or("", String::as_str);
    [s0, tags, &out.head, s1, &out.body, s2]
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

/// Whether handlers are timed, for the dev log: in dev, but not in the
/// edge build, which has no clock to read.
fn timed() -> bool {
    crate::settings().dev && cfg!(not(target_arch = "wasm32"))
}

/// Runs a handler future, turning a panic into a 500 so one bad request
/// cannot take the connection (or anything else) down with it.
pub(crate) async fn catch<F: Future<Output = crate::Result<()>>>(f: F) -> crate::Result<()> {
    catch_made(|| f).await
}

/// [`catch`] of the future `make` makes. The future is made in place: a
/// future taken as an argument would be held twice over, as it came and
/// as it is polled, and moved in full each request.
async fn catch_made<F: Future<Output = crate::Result<()>>>(
    make: impl FnOnce() -> F,
) -> crate::Result<()> {
    let mut f = std::pin::pin!(make());
    let timed = timed();
    std::future::poll_fn(move |cx| {
        let began = timed.then(Instant::now);
        IN_HANDLER.set(true);
        let polled = catch_unwind(AssertUnwindSafe(|| f.as_mut().poll(cx)));
        IN_HANDLER.set(false);
        if let Some(began) = began {
            BLOCKED.set(BLOCKED.get().max(began.elapsed()));
        }
        polled.unwrap_or_else(|panic| Poll::Ready(Err(panicked(panic))))
    })
    .await
}

/// [`catch`] of a sync [`App::handle_now`]: `None` when it answered
/// nothing.
#[cfg(target_os = "linux")]
fn catch_now(f: impl FnOnce() -> crate::Result<bool>) -> Option<crate::Result<()>> {
    let began = timed().then(Instant::now);
    IN_HANDLER.set(true);
    let ran = catch_unwind(AssertUnwindSafe(f));
    IN_HANDLER.set(false);
    if let Some(began) = began {
        BLOCKED.set(BLOCKED.get().max(began.elapsed()));
    }
    match ran {
        Ok(Ok(true)) => Some(Ok(())),
        Ok(Ok(false)) => None,
        Ok(Err(e)) => Some(Err(e)),
        Err(panic) => Some(Err(panicked(panic))),
    }
}

/// The 500 of a handler's `panic`, saying where it was.
fn panicked(panic: Box<dyn std::any::Any + Send>) -> Error {
    let msg = panic
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".into());
    let msg = match PANICKED_AT.take() {
        Some(at) => format!("panic at {at}: {msg}"),
        None => format!("panic: {msg}"),
    };
    Error::new(500, msg)
}

/// Wisp's own addresses: the browser runtime, the API docs and, in dev,
/// the dev tools. `false` for any other path.
fn internal<A: App>(cx: &Cx, path: &str, reply: &mut Reply) -> bool {
    if !path.starts_with("/_") {
        return false;
    }
    let s = crate::settings();
    let get = matches!(cx.method, Method::Get | Method::Head);
    let dev = get && s.dev;
    let docs = get && s.api_docs && !A::openapi().is_empty();
    let (body, ext, etag): (&'static [u8], _, _) = match path {
        crate::protocol::WISP_JS_PATH if get => (CLIENT_JS, "js", Some(CLIENT_JS_ETAG)),
        crate::protocol::LIVE_JS_PATH if get => (LIVE_JS, "js", Some(CLIENT_JS_ETAG)),
        "/_app/wisp-dev.js" if dev => (DEV_JS, "js", None),
        "/_app/wisp-ui.css" if dev => (UI_CSS.as_bytes(), "css", None),
        "/_app/wisp-dialog.css" if dev => (DIALOG_CSS, "css", None),
        "/_wisp/openapi.json" if docs => (A::openapi().as_bytes(), "json", None),
        "/_wisp/client.ts" if docs => (A::client_ts().as_bytes(), "txt", None),
        "/_wisp/docs" if docs => (api_docs(), "html", None),
        _ if s.dev && path.starts_with("/_wisp/") => {
            let (status, msg) = dev::endpoint::<A>(cx.method, path, cx.body(), cx.peer());
            reply.set_plain(status, msg);
            return true;
        }
        _ => return false,
    };
    send_file(reply, cx, Body::Static(body), ext, etag);
    true
}

/// The app's files: templates' browser modules, then its assets, embedded
/// or, in dev, read from `static/`. `route`: the route the path matches.
/// `false` if the path is not a file.
fn file<A: App>(cx: &Cx, raw: &[u8], route: Option<usize>, reply: &mut Reply) -> bool {
    let routed = route.is_some();
    // Compiled in, in dev too: a change to one is a rebuild anyway.
    if raw.starts_with(crate::protocol::MODULES.as_bytes())
        && let Some(m) = A::client_module(cx.path())
    {
        send_file(
            reply,
            cx,
            Body::Static(m.source.as_bytes()),
            "js",
            Some(m.etag),
        );
        return true;
    }
    if crate::settings().dev {
        let path = cx.path();
        // A page's path goes to the disk only if `static/` had a file there.
        if path != crate::protocol::APP_CSS_PATH
            && routed
            && !dev::listed(A::ROOT, &decode(path.as_bytes(), false))
        {
            return false;
        }
        let Some((bytes, ext)) = dev::read_file(A::ROOT, path) else {
            return false;
        };
        send_file(reply, cx, Body::Bytes(bytes), &ext, None);
        return true;
    }
    // The build knows which routes no embedded file is at.
    if route.is_some_and(|r| !A::ROUTES[r].files) {
        return false;
    }
    let Some(a) = A::asset(cx.path()) else {
        return false;
    };
    send_file(reply, cx, Body::Static(a.body), a.ext, Some(a.etag));
    true
}

/// A file, cached by its `etag` (forever when the address is versioned
/// with `?v=`), or never without one.
fn send_file(reply: &mut Reply, cx: &Cx, body: Body, ext: &str, etag: Option<&'static str>) {
    let cache = match etag {
        None => "no-store",
        Some(_) if cx.query_string().split('&').any(|kv| kv.starts_with("v=")) => {
            "public, max-age=31536000, immutable"
        }
        Some(_) => "public, max-age=0, must-revalidate",
    };
    if etag.is_some_and(|tag| fresh(cx, tag)) {
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
/// Chunks made meanwhile go out in one write. Anything the client sends
/// meanwhile is kept in `early`, up to a limit.
#[cfg(not(target_arch = "wasm32"))]
async fn pump(
    stream: &mut Conn,
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
    // An empty chunk would read as the end: it is left out.
    let put = |w: &mut Vec<u8>, chunk: &[u8]| {
        if chunked && !chunk.is_empty() {
            push_hex(w, chunk.len() as u64);
            w.extend_from_slice(b"\r\n");
            w.extend_from_slice(chunk);
            w.extend_from_slice(b"\r\n");
        } else {
            w.extend_from_slice(chunk);
        }
    };
    let mut stop = std::pin::pin!(stopped());
    loop {
        let next = first(
            async { Next::Chunk(body.recv().await) },
            first(
                async {
                    stop.as_mut().await;
                    Next::Stop
                },
                async {
                    loop {
                        if early.len() >= KEEP_CAPACITY {
                            std::future::pending::<()>().await;
                        }
                        match stream.read(early).await {
                            Ok(0) | Err(_) => return Next::Gone,
                            Ok(_) => {}
                        }
                    }
                },
            ),
        )
        .await;
        w.clear();
        let mut end = match next {
            Next::Chunk(Some(chunk)) => {
                put(w, &chunk);
                false
            }
            Next::Chunk(None) | Next::Stop => true,
            Next::Gone => return Err(io::ErrorKind::ConnectionAborted.into()),
        };
        while !end && w.len() < KEEP_CAPACITY {
            match body.try_recv() {
                Ok(chunk) => put(w, &chunk),
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => end = true,
            }
        }
        if end && chunked {
            w.extend_from_slice(b"0\r\n\r\n");
        }
        if !w.is_empty() {
            stream.write(w).await?;
        }
        if end {
            return Ok(());
        }
    }
}

/// A header the app set with a line break or NUL in it (through `Response`'s
/// public fields, which nothing checks) would split the response: it is
/// left out. A pair of `'static` strings (the code's own, as
/// `content-type: text/plain` is) is checked once a thread while it is one
/// of the last few such pairs: such a string never changes, so its address
/// and length say it is the same.
fn header(w: &mut Vec<u8>, (name, value): &(Cow<'static, str>, Cow<'static, str>)) {
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
fn type_line(value: &str) -> Option<&'static [u8]> {
    Some(match value {
        "text/plain; charset=utf-8" => b"content-type: text/plain; charset=utf-8\r\n",
        "text/html; charset=utf-8" => b"content-type: text/html; charset=utf-8\r\n",
        "application/json" => b"content-type: application/json\r\n",
        _ => return None,
    })
}

/// `n` in lowercase hex, as few digits as it takes.
fn push_hex(w: &mut Vec<u8>, n: u64) {
    let digits = (16 - n.leading_zeros() as usize / 4).max(1);
    for k in (0..digits).rev() {
        w.push(b"0123456789abcdef"[(n >> (4 * k) & 15) as usize]);
    }
}

fn push_decimal(w: &mut Vec<u8>, n: u64) {
    let mut buf = [0u8; 20];
    let start = crate::digits(&mut buf, 20, n);
    w.extend_from_slice(&buf[start..]);
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

/// The unix second: the clock's, when the server keeps one, else the
/// system's (a host other than the built-in server).
pub(crate) fn now() -> u64 {
    match NOW.load(Ordering::Relaxed) {
        0 => crate::unix_now(),
        n => n,
    }
}

/// When `seconds()` was 0.
static START: OnceLock<Instant> = OnceLock::new();

/// The instant of `deadline`, in `seconds()`, for a tokio timer.
#[cfg(not(target_arch = "wasm32"))]
fn instant(deadline: u64) -> tokio::time::Instant {
    let start = *START.get_or_init(Instant::now);
    tokio::time::Instant::from_std(start + Duration::from_secs(deadline))
}

pub(crate) fn start_clock() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        NOW.store(crate::unix_now(), Ordering::Relaxed);
        let start = *START.get_or_init(Instant::now);
        std::thread::Builder::new()
            .name("wisp-clock".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                    NOW.store(crate::unix_now(), Ordering::Relaxed);
                    SECONDS.store(start.elapsed().as_secs(), Ordering::Relaxed);
                }
            })
            .expect("failed to start clock thread");
    });
}

/// `date: Sun, 06 Nov 1994 08:49:37 GMT\r\n`
const DATE_LINE: usize = 37;

thread_local! {
    /// (unix second, its `date` line), formatted at most once a second.
    static DATE: Cell<(u64, [u8; DATE_LINE])> = const { Cell::new((u64::MAX, [0; DATE_LINE])) };
}

/// `content-length`, when the response has a `length`, and `date`, in one
/// piece: the digits are written right to left, ending where the date
/// line, kept formatted, begins.
fn length_and_date(w: &mut Vec<u8>, length: Option<usize>) {
    const PREFIX: &[u8] = b"content-length: ";
    const LENGTH_LINE: usize = PREFIX.len() + 20 + 2;
    let mut head = [0u8; LENGTH_LINE + DATE_LINE];
    head[LENGTH_LINE..].copy_from_slice(&date_line());
    let mut start = LENGTH_LINE;
    if let Some(n) = length {
        head[LENGTH_LINE - 2..LENGTH_LINE].copy_from_slice(b"\r\n");
        start = crate::digits(&mut head, LENGTH_LINE - 2, n as u64) - PREFIX.len();
        head[start..start + PREFIX.len()].copy_from_slice(PREFIX);
    }
    w.extend_from_slice(&head[start..]);
}

fn date_line() -> [u8; DATE_LINE] {
    let now = NOW.load(Ordering::Relaxed);
    DATE.with(|c| {
        let (secs, line) = c.get();
        if secs == now {
            return line;
        }
        let mut line = [0; DATE_LINE];
        line[..6].copy_from_slice(b"date: ");
        line[6..35].copy_from_slice(&http_date(now));
        line[35..].copy_from_slice(b"\r\n");
        c.set((now, line));
        line
    })
}

/// IMF-fixdate, e.g. `Sun, 06 Nov 1994 08:49:37 GMT`.
fn http_date(secs: u64) -> [u8; 29] {
    const DAYS: [&[u8; 3]; 7] = [b"Thu", b"Fri", b"Sat", b"Sun", b"Mon", b"Tue", b"Wed"];
    const MONTHS: [&[u8; 3]; 12] = [
        b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct", b"Nov",
        b"Dec",
    ];
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (year, month, day) = crate::civil(days);

    let two = |n: u64| [b'0' + (n / 10) as u8, b'0' + (n % 10) as u8];
    let mut out = [0u8; 29];
    out[..3].copy_from_slice(DAYS[(days % 7) as usize]);
    out[3..5].copy_from_slice(b", ");
    out[5..7].copy_from_slice(&two(day));
    out[7] = b' ';
    out[8..11].copy_from_slice(MONTHS[month as usize - 1]);
    out[11] = b' ';
    out[12..14].copy_from_slice(&two(year / 100 % 100));
    out[14..16].copy_from_slice(&two(year % 100));
    out[16] = b' ';
    out[17..19].copy_from_slice(&two(rem / 3600));
    out[19] = b':';
    out[20..22].copy_from_slice(&two(rem / 60 % 60));
    out[22] = b':';
    out[23..25].copy_from_slice(&two(rem % 60));
    out[25..29].copy_from_slice(b" GMT");
    out
}

/// The statuses Wisp names: the reason phrase of each, and its whole
/// status line, written in one piece.
macro_rules! statuses {
    ($($code:literal $reason:literal,)*) => {
        pub(crate) fn reason(status: u16) -> &'static str {
            match status {
                $($code => $reason,)*
                _ => "",
            }
        }

        /// `HTTP/1.1 200 OK\r\n`; `None` for a status not named here.
        fn status_line(status: u16) -> Option<&'static [u8]> {
            Some(match status {
                $($code => concat!("HTTP/1.1 ", $code, " ", $reason, "\r\n").as_bytes(),)*
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
        for n in [0, 9, 0x10, 0xabc, u64::MAX] {
            push_hex(&mut v, n);
            v.push(b' ');
        }
        assert_eq!(v, b"0 9 10 abc ffffffffffffffff ");
        v.clear();
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
        let mut put = |n: &'static str, v: &'static str| {
            header(&mut w, &(n.into(), v.into())); // checked, then known
            header(&mut w, &(n.into(), v.to_string().into())); // checked
        };
        put("x-a", "1");
        put("x-b", "2\r\nset-cookie: x=1");
        put("x-c\n", "3");
        put("x-d", "4\0");
        assert_eq!(w, b"x-a: 1\r\nx-a: 1\r\n");
    }

    #[test]
    fn paths() {
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
        cx.wire.buf.extend_from_slice(bytes);
        cx
    }

    /// What a parsed request says must lie inside the buffer, within limits.
    fn check_parsed(cx: &Cx, at: usize, len: usize) {
        assert!(at + len <= cx.wire.buf.len());
        let body = cx.wire.body.range();
        assert!(body.start >= at && body.end <= at + len);
        assert!(body.len() <= limit(cx.path()));
        for (n, v) in &cx.wire.headers {
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
            match parse::<Fuzz>(&mut cx, 0, false) {
                Parsed::Request(Req { len, .. }) if !too_large => {
                    assert_eq!(len, s.wire.len());
                    assert_eq!(
                        (cx.path(), cx.query_string(), cx.body()),
                        (s.path, &*s.query, &s.body[..])
                    );
                    assert_eq!(cx.wire.headers.len(), s.headers);
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
                match parse::<Fuzz>(&mut cx, 0, false) {
                    Parsed::Partial { .. } => {}
                    Parsed::Invalid(413) if too_large => {}
                    _ => panic!("{cut} of {:?}", String::from_utf8_lossy(&s.wire)),
                }
            }
        }
    }

    /// What `fast_head` reads, httparse reads the same, byte for byte; and
    /// it reads every well-formed request `sent` makes, so it is the path
    /// taken.
    #[test]
    fn the_fast_head_reads_as_httparse_does() {
        let same = |wire: &[u8], must: bool| {
            let (mut fast, mut slow) = (Vec::new(), Vec::new());
            let Some(f) = fast_head(wire, 0, &mut fast) else {
                assert!(!must, "{:?}", String::from_utf8_lossy(wire));
                return;
            };
            let Ok(s) = slow_head(wire, 0, &mut slow) else {
                panic!("httparse refused {:?}", String::from_utf8_lossy(wire));
            };
            let text = |s: Span| &wire[s.range()];
            let pairs = |h: &[(Span, Span)]| {
                h.iter()
                    .map(|&(n, v)| (text(n), text(v)))
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                (
                    f.len,
                    f.method,
                    f.http11,
                    text(f.path),
                    text(f.query),
                    pairs(&fast)
                ),
                (
                    s.len,
                    s.method,
                    s.http11,
                    text(s.path),
                    text(s.query),
                    pairs(&slow)
                ),
                "{:?}",
                String::from_utf8_lossy(wire)
            );
        };
        let mut rng = Rng::new(6);
        for k in 0..30_000 {
            let mut wire = sent(&mut rng).wire;
            let whole = k % 3 == 0;
            if !whole {
                mutate(&mut rng, &mut wire);
            }
            same(&wire, whole);
        }
        // Every byte in a method, target, header name and value, and around
        // the lines' ends.
        for v in 0..=255u8 {
            let c = char::from(v);
            for wire in [
                format!("G{c}T / HTTP/1.1\r\n\r\n").into_bytes(),
                format!("GET /a{c}b HTTP/1.1\r\n\r\n").into_bytes(),
                format!("GET / HTTP/1.1\r\nx{c}y: 1\r\n\r\n").into_bytes(),
                format!("GET / HTTP/1.1\r\nx:{c}a\r\n\r\n").into_bytes(),
                format!("GET / HTTP/1.1\r\nx: 0123456789{c}abcdef{c} \r\n\r\n").into_bytes(),
                format!("GET / HTTP/1.1{c}\nx: 1\r{c}\r\n").into_bytes(),
                [
                    &b"GET / HTTP/1.1\r\nx: "[..],
                    &[v, b'\r', b'\n', b'\r', b'\n'],
                ]
                .concat(),
                [
                    &b"GET /"[..],
                    &[v; 9],
                    b" HTTP/1.0\r\nx: ",
                    &[v; 17],
                    b"\r\n\r\n",
                ]
                .concat(),
            ] {
                same(&wire, false);
            }
        }
        same(
            b"GET /x?a=1&b HTTP/1.1\r\nHost: a\r\nX-Empty:\r\nX-Tabs:\t a\tb \t\r\n\r\n",
            true,
        );
        same(b"DELETE /x HTTP/1.0\r\n\r\n", true);
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
            let Parsed::Request(Req { len, .. }) = parse::<Fuzz>(&mut cx, 0, false) else {
                panic!()
            };
            assert_eq!((len, cx.body()), (a.wire.len(), &a.body[..]));
            let Parsed::Request(Req { len, .. }) = parse::<Fuzz>(&mut cx, len, false) else {
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
            match parse::<Fuzz>(&mut cx, 0, false) {
                Parsed::Request(Req { len, .. }) => {
                    check_parsed(&cx, 0, len);
                    rt.block_on(decide::<Fuzz>(&mut cx, &mut out, &mut reply, None));
                    w.clear();
                    serialize::<Fuzz>(&mut w, &mut reply, &mut out, cx.wire.http11, true, false);
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
            parse::<Fuzz>(&mut cx_with(&long), 0, false),
            Parsed::Invalid(431)
        ));
        let many: String = (0..MAX_HEADERS + 1)
            .map(|i| format!("x-{i}: 1\r\n"))
            .collect();
        let many = format!("GET / HTTP/1.1\r\n{many}\r\n");
        assert!(matches!(
            parse::<Fuzz>(&mut cx_with(many.as_bytes()), 0, false),
            Parsed::Invalid(431)
        ));

        let head = "POST /small HTTP/1.1\r\ntransfer-encoding: chunked\r\n\r\n";
        let body_of = |chunked: &str| {
            let mut cx = cx_with(format!("{head}{chunked}").as_bytes());
            match parse::<Fuzz>(&mut cx, 0, false) {
                Parsed::Request(_) => Ok(cx.body().to_vec()),
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
    fn http_1_1_names_one_host() {
        // On the wire; from another host (`Cx::from_request`) it may not.
        let hosted = |wire: &[u8], on_wire| match parse::<Fuzz>(&mut cx_with(wire), 0, on_wire) {
            Parsed::Request(_) => Ok(()),
            Parsed::Invalid(status) => Err(status),
            Parsed::Partial { .. } => Err(0),
        };
        assert_eq!(hosted(b"GET / HTTP/1.1\r\nHost: a\r\n\r\n", true), Ok(()));
        assert_eq!(hosted(b"GET / HTTP/1.1\r\nx: 1\r\n\r\n", true), Err(400));
        assert_eq!(hosted(b"GET / HTTP/1.1\r\nx: 1\r\n\r\n", false), Ok(()));
        assert_eq!(hosted(b"GET / HTTP/1.0\r\n\r\n", true), Ok(()));
        assert_eq!(
            hosted(b"GET / HTTP/1.1\r\nhost: a\r\nHOST: a\r\n\r\n", false),
            Err(400)
        );
        assert_eq!(
            hosted(b"GET / HTTP/1.0\r\nhost: a\r\nhost: b\r\n\r\n", true),
            Err(400)
        );
    }

    #[test]
    fn an_absolute_target_is_its_path_on_its_host() {
        let read = |wire: &[u8]| {
            let mut cx = cx_with(wire);
            match parse::<Fuzz>(&mut cx, 0, true) {
                Parsed::Request(_) => Some(format!(
                    "{} {} {}",
                    cx.header("host").unwrap_or("-"),
                    cx.path(),
                    cx.query_string()
                )),
                _ => None,
            }
        };
        let host =
            |target: &str| read(format!("GET {target} HTTP/1.1\r\nHost: proxy\r\n\r\n").as_bytes());
        assert_eq!(
            host("http://a.test/x/y?q=1").as_deref(),
            Some("a.test /x/y q=1")
        );
        assert_eq!(
            host("HTTPS://a.test:8443").as_deref(),
            Some("a.test:8443 / ")
        );
        assert_eq!(host("http://a.test?q").as_deref(), Some("a.test / q"));
        // Anything else is left as it came, and refused later.
        for odd in [
            "*",
            "ftp://a/",
            "http://",
            "http://u@a/",
            "http:/a/",
            "a.test/x",
        ] {
            assert_eq!(host(odd), Some(format!("proxy {odd} ")), "{odd}");
        }
        // HTTP/1.1 still names a host, as it must.
        assert_eq!(read(b"GET http://a/ HTTP/1.1\r\n\r\n"), None);
        assert_eq!(
            read(b"GET http://a/ HTTP/1.0\r\n\r\n").as_deref(),
            Some("- / ")
        );
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
            assert!(matches!(
                parse::<Fuzz>(&mut cx, 0, false),
                Parsed::Request(_)
            ));
            rt.block_on(decide::<Fuzz>(&mut cx, &mut out, &mut reply, None));
            assert_eq!((reply.status, reply.header("location")), (308, Some(to)));
        }

        let mut w = Vec::new();
        let mut reply = Reply::plain(200);
        for (n, v) in [("content-length", "99"), ("Transfer-Encoding", "chunked")] {
            reply.headers.push((Cow::Borrowed(n), Cow::Borrowed(v)));
        }
        serialize::<Fuzz>(&mut w, &mut reply, &mut out, false, true, false);
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

    /// The routes of `bench/app`, as `wisp-build` writes them: `/plaintext`
    /// (0), `/json` (1) and `/fortunes` (2), a page of escaped rows.
    struct Bench;

    struct Message {
        message: &'static str,
    }

    impl crate::Json for Message {
        fn json(&self, out: &mut String) {
            out.push_str("{\"message\":");
            crate::Json::json(&self.message, out);
            out.push('}');
        }
    }

    const ROWS: [(u32, &str); 13] = [
        (0, "Additional fortune added at request time."),
        (1, "fortune: No such file or directory"),
        (
            2,
            "A computer scientist is someone who fixes things that aren't broken.",
        ),
        (3, "After enough decimal places, nobody gives a damn."),
        (
            4,
            "A bad random number generator: 1, 1, 1, 1, 1, 4.33e+67, 1, 1, 1",
        ),
        (
            5,
            "A computer program does what you tell it to do, not what you want it to do.",
        ),
        (
            6,
            "Emacs is a nice operating system, but I prefer UNIX. — Tom Christaensen",
        ),
        (7, "Any program that runs right is obsolete."),
        (
            8,
            "A list is only as strong as its weakest link. — Donald Knuth",
        ),
        (9, "Feature: A bug with seniority."),
        (10, "Computers make very fast, very accurate mistakes."),
        (
            11,
            "<script>alert(\"This should not be displayed in a browser alert box.\");</script>",
        ),
        (12, "フレームワークのベンチマーク"),
    ];

    impl App for Bench {
        const ROOT: &'static str = ".";
        const CSS: Option<&'static str> = None;
        const ROUTES: &'static [crate::rt::RouteFacts] =
            &[const { crate::rt::RouteFacts::new(&[]) }; 3];
        const TEMPLATES: &'static [(&'static str, u64)] = &[];

        fn route(path: &str) -> Option<(usize, [&str; 8])> {
            let mut segs = [""; crate::rt::MAX_SEGS];
            let id = match crate::rt::split(path, &mut segs)? {
                ["plaintext"] => 0,
                ["json"] => 1,
                ["fortunes"] => 2,
                _ => return None,
            };
            Some((id, [""; 8]))
        }

        fn shell() -> [&'static str; 3] {
            [
                "<!DOCTYPE html>\n<html>\n<head>",
                "</head>\n<body>",
                "</body>\n</html>\n",
            ]
        }

        fn asset(_: &str) -> Option<&'static crate::Asset> {
            None
        }

        async fn init() -> crate::Result<()> {
            Ok(())
        }

        #[allow(clippy::needless_borrow)] // the form the generated code has
        async fn handle(route: Option<usize>, cx: &mut Cx, out: &mut Out) -> crate::Result<()> {
            use crate::html::{Direct as _, Text};
            use crate::rt_traits::ret::{Ret, Shape as _, Value as _};
            crate::rt::hooked(cx);
            let Some(route) = route else {
                return Err(Error::new(404, "Not Found"));
            };
            if cx.method == Method::Get && cx.header(crate::protocol::HEADER_ERROR).is_some() {
                return Err(Error::new(500, "Something went wrong in the browser"));
            }
            match route {
                0 => {
                    crate::rt::endpoint(cx);
                    let r = (&&&Ret::new(crate::Response::text("Hello, World!"))).respond()?;
                    crate::rt::respond(out, r);
                }
                1 => {
                    crate::rt::endpoint(cx);
                    let r = (&&&Ret::new(Message {
                        message: "Hello, World!",
                    }))
                        .respond()?;
                    crate::rt::respond(out, r);
                }
                _ => {
                    out.head.push_str("<title>Fortunes</title>");
                    out.body
                        .push_str("\n<table>\n<tr><th>id</th><th>message</th></tr>\n");
                    for (id, message) in &ROWS {
                        out.body.push_str("<tr><td>");
                        (&Text(id)).put(&mut out.body);
                        out.body.push_str("</td><td>");
                        (&Text(message)).put(&mut out.body);
                        out.body.push_str("</td></tr>\n");
                    }
                    out.body.push_str("</table>");
                }
            }
            Ok(())
        }

        async fn error(
            _: Option<usize>,
            cx: &mut Cx,
            out: &mut Out,
            status: u16,
            message: &str,
        ) -> crate::Result<()> {
            crate::rt::default_error(cx, out, status, message);
            Ok(())
        }
    }

    /// Polls `f` once: the routes of `Bench` never wait.
    fn ready<F: Future>(f: F) -> F::Output {
        let mut f = std::pin::pin!(f);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        match f.as_mut().poll(&mut cx) {
            Poll::Ready(v) => v,
            Poll::Pending => panic!("a Bench route waited"),
        }
    }

    /// One request on a kept-alive connection, as `requests` answers it:
    /// parsed from the read buffer, decided, written to the write buffer.
    fn answer(b: &mut Buffers, request: &[u8]) {
        b.cx.wire.buf.clear();
        b.cx.wire.buf.extend_from_slice(request);
        let Parsed::Request(Req { keep_alive, .. }) = parse::<Bench>(&mut b.cx, 0, false) else {
            panic!("{:?}", String::from_utf8_lossy(request));
        };
        ready(decide::<Bench>(&mut b.cx, &mut b.out, &mut b.reply, None));
        b.wbuf.clear();
        let head = b.cx.method == Method::Head;
        serialize::<Bench>(
            &mut b.wbuf,
            &mut b.reply,
            &mut b.out,
            b.cx.wire.http11,
            keep_alive,
            head,
        );
        b.cx.reset();
    }

    fn buffers() -> Buffers {
        Buffers {
            cx: Cx::new(SocketAddr::from(([127, 0, 0, 1], 1))),
            wbuf: Vec::with_capacity(16 * 1024),
            out: Out::default(),
            reply: Reply::default(),
        }
    }

    /// Once warm, a kept-alive connection answers `/plaintext`, `/json` and
    /// a page of escaped rows, HEAD or GET, with no allocation: its
    /// buffers, `Cx`, headers and response bodies are all reused. Measured
    /// as a release server runs, with dev mode off: in a child process, when
    /// this one has it on.

    #[test]
    fn warm_requests_allocate_nothing() {
        if crate::settings().dev {
            let name = "http::tests::warm_requests_allocate_nothing";
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([name, "--exact", "--nocapture"])
                .env("WISP_DEV", "off")
                .output()
                .unwrap();
            let said = String::from_utf8_lossy(&child.stdout);
            assert!(
                child.status.success() && said.contains("1 passed"),
                "{said}"
            );
            return;
        }
        let mut b = buffers();
        for (method, path, answered) in [
            ("GET", "/plaintext", &b"Hello, World!"[..]),
            ("GET", "/json", br#"{"message":"Hello, World!"}"#),
            ("GET", "/fortunes", b"&lt;script&gt;alert(&quot;This"),
            ("HEAD", "/json", b"content-length: 27\r\n"),
        ] {
            let request =
                format!("{method} {path} HTTP/1.1\r\nhost: localhost\r\naccept: */*\r\n\r\n");
            for _ in 0..3 {
                answer(&mut b, request.as_bytes());
            }
            let made = allocation_counter::measure(|| {
                for _ in 0..100 {
                    answer(&mut b, request.as_bytes());
                }
            });
            let text = String::from_utf8_lossy(&b.wbuf);
            assert!(text.starts_with("HTTP/1.1 200 OK\r\n"), "{text}");
            assert!(
                b.wbuf.windows(answered.len()).any(|w| w == answered),
                "{text}"
            );
            assert_eq!(made.count_total, 0, "{method} {path}");
        }
    }

    /// What a request costs Wisp itself, sockets aside: parse, decide and
    /// serialize on a warm connection, as zrk sends it.
    /// `cargo test -p wisp --release --lib http::tests::cost -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn cost() {
        let mut b = buffers();
        for path in ["/plaintext", "/json", "/fortunes"] {
            let request = format!(
                "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:3000\r\nUser-Agent: zrk\r\nConnection: keep-alive\r\n\r\n"
            );
            for _ in 0..10_000 {
                answer(&mut b, request.as_bytes());
            }
            let n = 1_000_000;
            let started = std::time::Instant::now();
            for _ in 0..n {
                answer(&mut b, std::hint::black_box(request.as_bytes()));
            }
            let ns = started.elapsed().as_nanos() as f64 / n as f64;
            println!("{path}: {ns:.0} ns a request");
        }
    }

    /// Routes the build said never wait: `/now` (0), and `/wait` (1),
    /// which waits after all, as a wrong guess would.
    #[cfg(target_os = "linux")]
    struct Guessed;

    #[cfg(target_os = "linux")]
    static WAITED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    #[cfg(target_os = "linux")]
    impl App for Guessed {
        const ROOT: &'static str = ".";
        const CSS: Option<&'static str> = None;
        const ROUTES: &'static [crate::rt::RouteFacts] = &[const {
            crate::rt::RouteFacts {
                now: true,
                ..crate::rt::RouteFacts::new(&[])
            }
        }; 2];
        const TEMPLATES: &'static [(&'static str, u64)] = &[];

        fn route(path: &str) -> Option<(usize, [&str; 8])> {
            match path {
                "/now" => Some((0, [""; 8])),
                "/wait" => Some((1, [""; 8])),
                _ => None,
            }
        }

        fn shell() -> [&'static str; 3] {
            ["", "", ""]
        }

        fn asset(_: &str) -> Option<&'static crate::Asset> {
            None
        }

        async fn init() -> crate::Result<()> {
            Ok(())
        }

        async fn handle(route: Option<usize>, cx: &mut Cx, out: &mut Out) -> crate::Result<()> {
            crate::rt::hooked(cx);
            crate::rt::endpoint(cx);
            let text = match route {
                Some(0) => "now",
                Some(_) => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    WAITED.fetch_add(1, Ordering::Relaxed);
                    "waited"
                }
                None => return Err(Error::new(404, "Not Found")),
            };
            crate::rt::respond(out, crate::Response::text(text));
            Ok(())
        }

        async fn error(
            _: Option<usize>,
            cx: &mut Cx,
            out: &mut Out,
            status: u16,
            message: &str,
        ) -> crate::Result<()> {
            crate::rt::default_error(cx, out, status, message);
            Ok(())
        }
    }

    /// A request to a route the build wrongly said never waits is no
    /// failure: the driver hands it, still deciding, to the connection's
    /// future, which answers it once, then the requests after it, in order;
    /// and the driver answers the next ones itself again.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_route_that_waits_after_all_is_answered_in_order() {
        use std::io::{Read, Write};
        let listener = crate::uring::listen("127.0.0.1:0".parse().unwrap()).unwrap();
        let addr = listener.local_addr().unwrap();
        start_clock();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let (ready, _) = std::sync::mpsc::channel();
                let now = Some(on_driver::<Guessed> as fn(u64) -> bool);
                tokio::spawn(crate::epoll::serve(listener, polled::<Guessed>, now, ready));
                std::future::pending::<()>().await
            });
        });
        let mut c = std::net::TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let get = |path: &str| format!("GET {path} HTTP/1.1\r\nhost: a\r\n\r\n");
        // The bodies of the answers, in order, each by its content-length.
        let mut ask = |paths: &[&str]| {
            let wire: String = paths.iter().map(|p| get(p)).collect();
            c.write_all(wire.as_bytes()).unwrap();
            let (mut got, mut bodies, mut buf) = (String::new(), Vec::new(), [0; 4096]);
            while bodies.len() < paths.len() {
                if let Some(end) = got.find("\r\n\r\n") {
                    let len: usize = got[..end]
                        .split("content-length: ")
                        .nth(1)
                        .and_then(|l| l.split("\r\n").next()?.parse().ok())
                        .unwrap();
                    if got.len() >= end + 4 + len {
                        assert!(got.starts_with("HTTP/1.1 200 OK"), "{got}");
                        bodies.push(got[end + 4..end + 4 + len].to_string());
                        got.drain(..end + 4 + len);
                        continue;
                    }
                }
                let n = c.read(&mut buf).unwrap();
                assert!(n > 0, "closed after {got:?}");
                got.push_str(std::str::from_utf8(&buf[..n]).unwrap());
            }
            bodies.join(" ")
        };
        let before = WAITED.load(Ordering::Relaxed);
        assert_eq!(ask(&["/now"]), "now");
        std::thread::sleep(Duration::from_millis(50)); // its future waits bare
        assert_eq!(ask(&["/now", "/wait", "/now"]), "now waited now");
        assert_eq!(WAITED.load(Ordering::Relaxed) - before, 1);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(ask(&["/now", "/now"]), "now now");
        assert_eq!(ask(&["/wait"]), "waited");
    }

    /// Routes whose GET the build made sync: `/sync` (0), which
    /// `handle_now` answers; `/later` (1), which it turns out not to, so
    /// `handle` does; `/boom` (2), which panics; `/gone` (3), which fails
    /// to its error page, rendered in the decider.
    #[cfg(target_os = "linux")]
    struct Plain;

    #[cfg(target_os = "linux")]
    impl App for Plain {
        const ROOT: &'static str = ".";
        const CSS: Option<&'static str> = None;
        const ROUTES: &'static [crate::rt::RouteFacts] = &[const {
            crate::rt::RouteFacts {
                now: true,
                sync: Method::Get.bit(),
                ..crate::rt::RouteFacts::new(&[])
            }
        }; 4];
        const TEMPLATES: &'static [(&'static str, u64)] = &[];

        fn route(path: &str) -> Option<(usize, [&str; 8])> {
            let id = ["/sync", "/later", "/boom", "/gone"]
                .iter()
                .position(|p| *p == path)?;
            Some((id, [""; 8]))
        }

        fn shell() -> [&'static str; 3] {
            ["", "", ""]
        }

        fn asset(_: &str) -> Option<&'static crate::Asset> {
            None
        }

        async fn init() -> crate::Result<()> {
            Ok(())
        }

        async fn handle(_: Option<usize>, _: &mut Cx, out: &mut Out) -> crate::Result<()> {
            crate::rt::respond(out, crate::Response::text("later"));
            Ok(())
        }

        async fn error(
            _: Option<usize>,
            _: &mut Cx,
            out: &mut Out,
            _: u16,
            _: &str,
        ) -> crate::Result<()> {
            out.body.push_str("page");
            Ok(())
        }

        fn handle_now(route: Option<usize>, _: &mut Cx, out: &mut Out) -> crate::Result<bool> {
            match route {
                Some(0) => crate::rt::respond(out, crate::Response::text("sync")),
                Some(2) => panic!("boom"),
                Some(3) => return Err(Error::new(410, "Gone")),
                _ => return Ok(false),
            }
            Ok(true)
        }
    }

    /// The driver answers sync arms with no future, falls back to `handle`
    /// for one that has none, turns a panic into a 500, and has the decider
    /// render an error page: all in order, none handed on.
    #[cfg(target_os = "linux")]
    #[test]
    fn sync_arms_are_answered_without_a_future() {
        start_clock();
        let mut b = Box::new(buffers());
        let paths = ["/sync", "/later", "/boom", "/gone", "/sync"];
        let wire =
            paths.map(|p| format!("GET {p} HTTP/1.1\r\nhost: a\r\naccept: text/html\r\n\r\n"));
        b.cx.wire.buf.extend_from_slice(wire.concat().as_bytes());
        let Ok(b) = answer_whole::<Plain>(b) else {
            panic!("handed on");
        };
        let text = String::from_utf8_lossy(&b.wbuf);
        let statuses: Vec<&str> = text.split("HTTP/1.1 ").skip(1).map(|r| &r[..3]).collect();
        assert_eq!(statuses, ["200", "200", "500", "410", "200"], "{text}");
        let bodies: Vec<&str> = (text.split("\r\n\r\n").skip(1))
            .map(|r| r.split("HTTP/1.1").next().unwrap())
            .collect();
        // A page's head may have tags other tests set (`HEAD_TAGS`).
        let ends = ["sync", "later", "page", "page", "sync"];
        assert!(
            bodies.iter().zip(ends).all(|(b, e)| b.ends_with(e)),
            "{text}"
        );
    }

    /// A `String` input takes a spare body only of about its size: one it
    /// keeps (in a table, say) never holds a large body's room.
    #[test]
    fn a_string_input_takes_a_spare_of_its_size_only() {
        recycle(Vec::with_capacity(4096));
        assert!(spare_for(3).capacity() < 64);
        assert_eq!(spare().capacity(), 4096);
        recycle(Vec::with_capacity(32));
        assert_eq!(spare_for(10).capacity(), 32);
        assert_eq!(spare().capacity(), 0);
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
                check_parsed(&cx, 0, cx.wire.buf.len());
            }
        }
    }
}
