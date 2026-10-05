//! Listening: run, the workers, binding, connection slots and stopping.

use super::*;

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

    let listener = match crate::setting::<u16>("WISP_PORT_TRIES", "a number of ports") {
        Some(tries) if tries > 0 && addr.port() != 0 => bind_near(addr, tries)?,
        _ => bind_after_exit(addr)?,
    };
    #[cfg(target_os = "linux")]
    return run_linux::<A>(&main, listener, threads.max(1));
    #[cfg(not(target_os = "linux"))]
    run_tokio::<A>(&main, listener, threads.max(1))
}

/// [`run`] where this thread accepts, on tokio's sockets.
#[cfg(not(any(target_arch = "wasm32", target_os = "linux")))]
pub(super) fn run_tokio<A: App>(
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
        let mut signal = std::pin::pin!(stop_signal());
        started(listener.local_addr()?);
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
pub(super) fn run_linux<A: App>(
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
                        (answers::<A>() && !crate::obs::on()).then_some(on_driver::<A>),
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
    let signal = {
        let _in = main.enter();
        stop_signal()
    };
    started(addr);
    main.block_on(async {
        signal.await;
        stop(&workers).await; // the workers close their listeners
        Ok(())
    })
}

/// Worker `i`: a thread with a single-threaded tokio runtime of its own,
/// which runs `main` (it never ends) and what is spawned on the handle.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn worker<F: Future<Output = ()>>(
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
pub(super) fn ringed<A: App>(stream: std::net::TcpStream) {
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
pub(super) fn polled<A: App>(stream: std::net::TcpStream) {
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
pub(super) fn admit(stream: &std::net::TcpStream) -> Option<(Slot, SocketAddr)> {
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
pub(super) async fn stop(workers: &[tokio::runtime::Handle]) {
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
/// Ctrl+C (SIGINT). On Unix both are caught from the call on, not from the
/// first poll: made before `listening` is said, a signal sent as soon as it
/// is (a test's, a supervisor's) stops the server rather than killing it
/// mid-response. Called within a runtime.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::manual_async_fn)] // eager on Unix, where the catching comes first
pub(super) fn stop_signal() -> impl Future<Output = ()> {
    #[cfg(unix)]
    let caught = {
        use tokio::signal::unix::{SignalKind, signal};
        (signal(SignalKind::terminate()).ok()).zip(signal(SignalKind::interrupt()).ok())
    };
    async move {
        #[cfg(unix)]
        if let Some((mut term, mut int)) = caught {
            let term = async {
                term.recv().await;
            };
            let int = async {
                int.recv().await;
            };
            return first(term, int).await;
        }
        if tokio::signal::ctrl_c().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
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
pub(super) static STOPPING: AtomicBool = AtomicBool::new(false);

/// Wakes what waits for the server to stop.
pub(super) static STOP: Notify = Notify::const_new();

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
pub(super) struct Busy(pub(super) bool);

impl Busy {
    pub(super) fn set(&mut self, busy: bool) {
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
pub(super) static CONNS: AtomicUsize = AtomicUsize::new(0);

/// One of `CONNS`, held for its connection's life.
#[cfg(not(target_arch = "wasm32"))]
pub(super) struct Slot;

#[cfg(not(target_arch = "wasm32"))]
impl Slot {
    /// A slot, or `None` at the cap. Counted before the check, so two at
    /// once cannot both take the last one; a refused one uncounts as it drops.
    pub(super) fn take(max: usize) -> Option<Slot> {
        let slot = Slot;
        (CONNS.fetch_add(1, Ordering::Relaxed) < max).then_some(slot)
    }
}

/// A connection over the cap: told 503 (its send buffer, new and empty,
/// takes it without waiting) and closed, before it costs a task or a read.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn refuse(mut s: &std::net::TcpStream) {
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
pub(super) struct Held(std::sync::Arc<[AtomicUsize]>, usize);

#[cfg(not(any(target_arch = "wasm32", target_os = "linux")))]
impl Drop for Held {
    fn drop(&mut self) {
        self.0[self.1].fetch_sub(1, Ordering::Relaxed);
    }
}

/// Binds, with what to do about the usual failures.
pub(super) fn bind(addr: SocketAddr) -> io::Result<std::net::TcpListener> {
    std::net::TcpListener::bind(addr).map_err(|e| cannot_listen(addr, e))
}

/// [`bind`], trying again for a moment when the port is taken: a server killed
/// a moment ago keeps its io_uring listeners until the kernel has torn its
/// ring down, and a restart must not lose that race. A copy that still runs
/// holds the port past the wait.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn bind_after_exit(addr: SocketAddr) -> io::Result<std::net::TcpListener> {
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

/// `wisp dev` asks for this (`WISP_PORT_TRIES`): binds `addr`, or when its
/// port is taken the next, up to `tries` more, and keeps the listener, so no
/// other program can take the port in between.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn bind_near(addr: SocketAddr, tries: u16) -> io::Result<std::net::TcpListener> {
    let last = addr.port().saturating_add(tries);
    for port in addr.port()..=last {
        match bind(SocketAddr::new(addr.ip(), port)) {
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => {}
            r => return r,
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AddrInUse,
        format!(
            "ports {} to {last} are all in use
  Stop what is using them, or set PORT to use another.",
            addr.port()
        ),
    ))
}

pub(super) fn cannot_listen(addr: SocketAddr, e: io::Error) -> io::Error {
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

/// Serves from inside a runtime the caller owns, until the future is
/// dropped. Stopping gracefully is up to the caller.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn serve<A: App>(addr: SocketAddr) -> io::Result<()> {
    crate::prepare::<A>().await?;
    serve_on::<A>(bind(addr)?).await
}

/// [`serve`] on a socket already bound, once [`crate::prepare`] has run:
/// `wisp::test::browser` binds port 0 to learn its port first.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn serve_on<A: App>(listener: std::net::TcpListener) -> io::Result<()> {
    listener.set_nonblocking(true)?;
    let listener = TcpListener::from_std(listener)?;
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
pub(super) fn started(addr: SocketAddr) {
    start_clock();
    dev::exit_with_parent();
    // `wisp dev` waits for this exact line to know the app is ready.
    let line = format!("wisp: listening on http://{addr}\n");
    let mut stdout = io::stdout().lock();
    let _ = stdout
        .write_all(line.as_bytes())
        .and_then(|()| stdout.flush());
}

/// What every host needs before the first request, whether or not it runs
/// the built-in server. Cheap to call again.
pub(crate) fn setup<A: App>() {
    install_panic_hook();
    crate::obs::init(A::ROUTES);
    let _ = crate::sign::ROOT.set(A::ROOT);
    crate::i18n::ready(A::LOCALES);
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
            env!("WISP_BASE"),
            "/_app/wisp.js",
            "?v=",
            env!("WISP_RUNTIME_V"),
            "\"></script>"
        ));
        if let Some(p) = A::PWA {
            s.push_str(p.head);
            crate::pwa::icons(p.icons);
        }
        if let Some(port) = dev::events_port() {
            s.push_str(&format!(
                "<script defer src=\"/_app/wisp-dev.js\" data-port=\"{port}\"></script>"
            ));
            if cfg!(debug_assertions) {
                s.push_str("<script defer src=\"/_app/wisp-devtools.js\"></script>");
            }
        }
        s
    });
}

/// Says a header or cookie the app set was not sent: its value could not
/// be one (CR/LF, or a character a cookie cannot hold). Cold, and one
/// place, so the checks cost the hot path a branch and no code.
#[cold]
#[inline(never)]
pub(crate) fn dropped(what: &str, name: &str, value: &str) {
    log(format_args!(
        "wisp: dropped {what} {name:?}={value:?}: a character it cannot hold"
    ));
}

/// A line on stderr. Unlike `eprintln!`, a log that cannot be written (its
/// reader gone) never takes a request down with it.
pub(crate) fn log(line: std::fmt::Arguments) {
    #[cfg(target_arch = "wasm32")]
    crate::edge::log(&line.to_string());
    #[cfg(not(target_arch = "wasm32"))]
    // One `write_all`, so a line is never split or interleaved with another.
    let _ = io::stderr()
        .lock()
        .write_all(format!("{line}\n").as_bytes());
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
