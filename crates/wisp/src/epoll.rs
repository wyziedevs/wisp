//! The Linux server's sockets where io_uring does not work (before Linux
//! 6.1, a container's seccomp profile, the `io_uring_disabled` sysctl, a
//! kernel that refuses buffer rings; see `uring::rings`) or `WISP_IO=epoll`
//! asks: `uring.rs`'s design on an epoll instead of a ring.
//! Each worker accepts on a listener of its own (`SO_REUSEPORT`, the kernel
//! spreads connections), and has an epoll of its own that its sockets are in,
//! edge-triggered, from their accept to their close.
//!
//! A connection receives and sends by itself, straight into and out of its
//! buffers: one `recv` and one `send` a request, and no call at all to learn
//! it has nothing more to read (a receive that got less than it asked for
//! emptied the socket). Only a send the socket has no room for is left to
//! the driver, which finishes it as room comes, dropped connection or not.
//!
//! A connection is a task (`spawn`), but its future lives in its entry, and
//! when its socket has a request for a connection that waits on it, the
//! driver polls the future right there, with the task's waker, instead of
//! waking the task: the request is received, answered and sent in the
//! driver's turn, and the scheduler never hears of it. Whatever the future
//! waits on then (a handler's timer or database, a send the socket had no
//! room for, a streamed body) wakes the task as it would have anyway, and
//! the task polls the same future; once it waits on its socket again, the
//! driver has it back.
//!
//! The epoll lives inside tokio, as the ring does: it is one more thing
//! tokio's epoll waits on, and one `epoll_wait` a turn of the worker's
//! driver hands its events out. So handlers await timers, channels and
//! database drivers as before. A WebSocket's socket is handed over to tokio.
//! Receive deadlines and stalled sends are the driver's: one pass over the
//! connections a second, not a timer each.
//!
//! Its `unsafe` blocks are the socket calls std has no word for, each with
//! why it holds.
#![allow(unsafe_code)]

use crate::http;
use crate::uring::{owned, yield_once};
use std::cell::RefCell;
use std::future::{Future, poll_fn};
use std::io;
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;
use tokio::time::Instant;

/// Events one `epoll_wait` takes. A turn that gets them all yields, and the
/// next one takes the rest.
const EVENTS: usize = 256;
/// The room a receive has at least.
const RECV_ROOM: usize = 4096;
/// The events of the listener. A connection's are its entry and generation
/// (see `token`).
const ACCEPT: u64 = u64::MAX;
/// What a connection's socket is watched for, from its accept on.
const WATCH: u32 = (libc::EPOLLIN | libc::EPOLLOUT | libc::EPOLLRDHUP | libc::EPOLLET) as u32;
const READABLE: u32 = (libc::EPOLLIN | libc::EPOLLRDHUP | libc::EPOLLHUP | libc::EPOLLERR) as u32;
const WRITABLE: u32 = (libc::EPOLLOUT | libc::EPOLLHUP | libc::EPOLLERR) as u32;
/// The peer's end, or an error: no event follows, so every receive tries.
const ENDED: u32 = (libc::EPOLLRDHUP | libc::EPOLLHUP | libc::EPOLLERR) as u32;

/// A connection's future, as `spawn` keeps it in its entry.
type Serve = Pin<Box<dyn Future<Output = ()>>>;

/// The events' `u64` for connection `id` in generation `generation` of its
/// entry: an event taken in the turn that closed a connection cannot reach
/// the next one in the same entry.
fn token(id: usize, generation: u32) -> u64 {
    (u64::from(generation) << 32) | id as u64
}

/// The error of the call that just failed.
fn errno() -> i32 {
    io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or(libc::EIO)
}

/// epoll_ctl(2): `op` for `fd`, with `events` and `data` for ADD.
fn ctl(epoll: RawFd, op: libc::c_int, fd: RawFd, events: u32, data: u64) -> io::Result<()> {
    let mut ev = libc::epoll_event { events, u64: data };
    // SAFETY: an epoll_event alive for the call, which the kernel only reads.
    if unsafe { libc::epoll_ctl(epoll, op, fd, &raw mut ev) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Receives onto the end of `buf`, into the room it has: the bytes, 0 at
/// the end of the stream, or the errno.
fn recv(fd: RawFd, buf: &mut Vec<u8>) -> Result<usize, i32> {
    let room = buf.spare_capacity_mut();
    let (at, len) = (room.as_mut_ptr(), room.len());
    loop {
        // SAFETY: recv(2) writes at most `len` bytes at `at`, the vector's
        // room, which nothing else refers to during the call.
        let n = unsafe { libc::recv(fd, at.cast(), len, 0) };
        if n >= 0 {
            // SAFETY: the kernel wrote the first `n` bytes of the room.
            unsafe { buf.set_len(buf.len() + n as usize) };
            return Ok(n as usize);
        }
        match errno() {
            libc::EINTR => {}
            e => return Err(e),
        }
    }
}

/// Sends what of `buf` the socket takes: how much, or the errno.
fn send(fd: RawFd, buf: &[u8]) -> Result<usize, i32> {
    loop {
        // SAFETY: send(2) reads `buf`, alive for the call. MSG_NOSIGNAL: a
        // peer gone is EPIPE, not SIGPIPE.
        let n = unsafe { libc::send(fd, buf.as_ptr().cast(), buf.len(), libc::MSG_NOSIGNAL) };
        match n {
            1.. => return Ok(n as usize),
            0 => return Err(libc::EPIPE),
            _ => match errno() {
                libc::EINTR => {}
                e => return Err(e),
            },
        }
    }
}

/// A connection as its worker keeps it: in an entry that outlives the
/// `Sock` for as long as a send it left is under way, or its future runs.
#[derive(Default)]
struct Entry {
    /// The socket, -1 once the `Sock` and its send are done.
    fd: RawFd,
    generation: u32,
    /// The socket may have something to receive (or its end, or an error):
    /// set by its events, cleared by a receive that got less than it asked
    /// for.
    readable: bool,
    /// An event said the peer ended or the socket failed: a short receive
    /// no longer means the socket is empty, as its end (or error) is still
    /// to be read and no event will say so again.
    ended: bool,
    /// The rest of a send the socket had no room for, from `sent` on, or
    /// (empty) the buffer the next such send swaps in.
    out: Vec<u8>,
    sent: usize,
    sending: bool,
    /// When the send last made progress, in `http::seconds()`.
    since: u64,
    /// A send failed with this error: every later call fails.
    failed: i32,
    waker: Option<Waker>,
    /// The connection waits for something to receive, until `deadline`
    /// (in `http::seconds()`, 0 for none), or for the send.
    wants_recv: bool,
    deadline: u64,
    wants_send: bool,
    /// The `Sock` is gone: the socket, closed once its send is done.
    close: Option<OwnedFd>,
    /// A future `spawn` started for the connection runs: the entry is not
    /// reused until it ends, socket or not.
    attached: bool,
    /// That future, while nothing polls it.
    serve: Option<Serve>,
    /// The waker of its task, which the driver polls it with.
    task: Option<Waker>,
}

impl Entry {
    fn wait(&mut self, cx: &Context) {
        if !self.waker.as_ref().is_some_and(|w| w.will_wake(cx.waker())) {
            self.waker = Some(cx.waker().clone());
        }
    }

    fn wake(&mut self) {
        (self.wants_recv, self.wants_send) = (false, false);
        if let Some(w) = &self.waker {
            w.wake_by_ref();
        }
    }

    fn result(&self) -> io::Result<()> {
        match self.failed {
            0 => Ok(()),
            e => Err(io::Error::from_raw_os_error(e)),
        }
    }
}

/// A worker's epoll and connections.
struct Worker {
    epoll: RawFd,
    conns: Vec<Entry>,
    free: Vec<usize>,
    events: Vec<libc::epoll_event>,
    /// The connections (`token`s) whose futures the driver polls after the
    /// turn: their sockets have what they wait for.
    ready: Vec<u64>,
    /// The second deadlines and sends were last checked (`tick`).
    checked: u64,
    /// The listener had an event: connections may wait in it.
    acceptable: bool,
}

thread_local! {
    /// This thread's worker, set once its driver starts.
    static WORKER: RefCell<Option<Worker>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Worker) -> R) -> R {
    WORKER.with_borrow_mut(|w| f(w.as_mut().expect("an epoll socket used off its worker")))
}

impl Worker {
    /// One `epoll_wait`, without waiting: hands out the events that came.
    /// True when it took all it could, so more may wait.
    fn turn(&mut self) -> bool {
        self.events.clear();
        // SAFETY: epoll_wait(2) writes at most EVENTS events into the
        // vector's room, which has that many.
        let n = unsafe {
            libc::epoll_wait(
                self.epoll,
                self.events.as_mut_ptr(),
                EVENTS as libc::c_int,
                0,
            )
        };
        if n < 0 {
            match errno() {
                libc::EINTR => return true,
                e => crate::fail(&format!(
                    "epoll stopped working: {}",
                    io::Error::from_raw_os_error(e)
                )),
            }
        }
        // SAFETY: the kernel wrote the first `n` events.
        unsafe { self.events.set_len(n as usize) };
        for i in 0..self.events.len() {
            let ev = self.events[i];
            let (events, data) = (ev.events, ev.u64);
            self.event(data, events);
        }
        if http::seconds() != self.checked {
            self.tick();
        }
        n as usize == EVENTS
    }

    fn event(&mut self, data: u64, events: u32) {
        if data == ACCEPT {
            self.acceptable = true;
            return;
        }
        let id = data as u32 as usize;
        let e = &mut self.conns[id];
        if e.fd < 0 || e.generation != (data >> 32) as u32 {
            return;
        }
        if events & READABLE != 0 {
            e.readable = true;
            e.ended |= events & ENDED != 0;
            if e.wants_recv {
                if e.task.is_some() {
                    // Polled by the driver after the turn, with the task's waker.
                    e.wants_recv = false;
                    self.ready.push(data);
                } else {
                    e.wake();
                }
            }
        }
        if events & WRITABLE != 0 && self.conns[id].sending {
            self.push(id);
        }
    }

    /// Sends what the socket takes of connection `id`'s rest.
    fn push(&mut self, id: usize) {
        let e = &mut self.conns[id];
        while e.sent < e.out.len() {
            match send(e.fd, &e.out[e.sent..]) {
                Ok(n) => (e.sent, e.since) = (e.sent + n, http::seconds()),
                Err(libc::EAGAIN) => return, // the rest when there is room
                Err(errno) => return self.sent(id, errno),
            }
        }
        self.sent(id, 0);
    }

    /// Connection `id`'s send is over: all of it went, or `failed`.
    fn sent(&mut self, id: usize, failed: i32) {
        let e = &mut self.conns[id];
        e.sending = false;
        e.failed = failed;
        e.out.clear();
        if e.out.capacity() > http::KEEP_CAPACITY {
            e.out.shrink_to(http::KEEP_CAPACITY);
        }
        // A failure is for a connection waiting to receive too: its `read` fails.
        if e.wants_send || (failed != 0 && e.wants_recv) {
            e.wake();
        }
        if e.close.is_some() {
            self.release(id);
        }
    }

    /// Once a second: fails the sends that made no progress for
    /// `WRITE_TIMEOUT` (their clients stopped taking the response, and are
    /// dropped as the tokio path drops them), and wakes the receives past
    /// their deadlines, which then fail.
    fn tick(&mut self) {
        let now = http::seconds();
        self.checked = now;
        for id in 0..self.conns.len() {
            let e = &mut self.conns[id];
            if e.fd < 0 {
                continue;
            }
            if e.sending && now >= e.since + http::WRITE_TIMEOUT.as_secs() {
                self.sent(id, libc::ETIMEDOUT);
            } else if e.wants_recv && e.deadline != 0 && now >= e.deadline {
                e.wake();
            }
        }
    }

    /// An entry for the socket `fd`, which its events reach from now on.
    fn open(&mut self, fd: RawFd) -> usize {
        let id = self.free.pop().unwrap_or_else(|| {
            self.conns.push(Entry::default());
            self.conns.len() - 1
        });
        let e = &mut self.conns[id];
        let mut out = std::mem::take(&mut e.out);
        out.clear();
        *e = Entry {
            fd,
            generation: e.generation.wrapping_add(1),
            // Its request is likely in already: the first read tries.
            readable: true,
            out,
            ..Entry::default()
        };
        if let Err(err) = ctl(
            self.epoll,
            libc::EPOLL_CTL_ADD,
            fd,
            WATCH,
            token(id, e.generation),
        ) {
            // Out of memory for the watch: the connection fails at once.
            e.failed = err.raw_os_error().unwrap_or(libc::EIO);
        }
        id
    }

    /// Entry `id` is done with its socket, which is closed if it still has
    /// it; free unless its future still runs.
    fn release(&mut self, id: usize) {
        let e = &mut self.conns[id];
        (e.fd, e.close, e.waker) = (-1, None, None);
        if !e.attached {
            self.free.push(id);
        }
    }

    /// Entry `id`'s future ended: free unless its socket is still in use.
    fn finish(&mut self, id: usize) {
        let e = &mut self.conns[id];
        (e.attached, e.serve, e.task) = (false, None, None);
        if e.fd < 0 {
            self.free.push(id);
        }
    }

    fn read(
        &mut self,
        id: usize,
        buf: &mut Vec<u8>,
        deadline: u64,
        cx: &Context,
    ) -> Poll<io::Result<usize>> {
        let e = &mut self.conns[id];
        e.result()?;
        if e.readable {
            buf.reserve(RECV_ROOM);
            let room = buf.capacity() - buf.len();
            match recv(e.fd, buf) {
                Ok(n) => {
                    // Less than it could take: the socket is empty until its
                    // next event, unless its end came with what was read.
                    e.readable = e.ended || n == room || n == 0;
                    return Poll::Ready(Ok(n));
                }
                Err(libc::EAGAIN) => e.readable = false,
                Err(errno) => return Poll::Ready(Err(io::Error::from_raw_os_error(errno))),
            }
        }
        if deadline != 0 && http::seconds() >= deadline {
            return Poll::Ready(Err(io::ErrorKind::TimedOut.into()));
        }
        (e.wants_recv, e.deadline) = (true, deadline);
        e.wait(cx);
        Poll::Pending
    }

    /// Sends all of `buf`, and leaves it empty: what the socket has no room
    /// for goes on from the driver (and `buf` gets the buffer the last such
    /// send used).
    fn write(&mut self, id: usize, buf: &mut Vec<u8>) -> io::Result<()> {
        let e = &mut self.conns[id];
        debug_assert!(!e.sending && e.out.is_empty());
        e.result()?;
        let mut at = 0;
        while at < buf.len() {
            match send(e.fd, &buf[at..]) {
                Ok(n) => at += n,
                Err(libc::EAGAIN) => {
                    std::mem::swap(&mut e.out, buf);
                    (e.sent, e.sending, e.since) = (at, true, http::seconds());
                    return Ok(());
                }
                Err(errno) => {
                    e.failed = errno;
                    buf.clear();
                    return e.result();
                }
            }
        }
        buf.clear();
        Ok(())
    }

    fn flushed(&mut self, id: usize, cx: &Context) -> Poll<io::Result<()>> {
        let e = &mut self.conns[id];
        if e.sending {
            e.wants_send = true;
            e.wait(cx);
            return Poll::Pending;
        }
        Poll::Ready(e.result())
    }

    /// The `Sock` of `id` is gone, with its socket unless tokio has that:
    /// closed now, or once the send under way is done.
    fn close(&mut self, id: usize, stream: Option<TcpStream>) {
        let e = &mut self.conns[id];
        (e.wants_recv, e.wants_send, e.waker) = (false, false, None);
        match stream {
            Some(s) if e.sending => e.close = Some(OwnedFd::from(s)),
            s => {
                drop(s);
                self.release(id);
            }
        }
    }
}

/// Polls a connection's future once, outside the worker's borrow (its
/// `Sock` borrows the worker): `Some` while it goes on. A panic ends the
/// connection, as it would end a task, and never the driver.
fn step(mut serve: Serve, cx: &mut Context) -> Option<Serve> {
    match catch_unwind(AssertUnwindSafe(|| serve.as_mut().poll(cx))) {
        Ok(Poll::Pending) => Some(serve),
        _ => None,
    }
}

/// Serves the socket `stream` with the future `serve` makes of it: a task,
/// whose future the driver also polls whenever the socket has what the
/// future waits for (see the top of this file). Called by the driver's
/// `accepted`.
pub(crate) fn spawn<F: Future<Output = ()> + 'static>(
    stream: TcpStream,
    serve: impl FnOnce(Sock) -> F,
) {
    let sock = Sock::new(stream);
    let id = sock.id;
    let serve: Serve = Box::pin(serve(sock));
    let generation = with(|w| {
        let e = &mut w.conns[id];
        (e.attached, e.serve) = (true, Some(serve));
        e.generation
    });
    tokio::spawn(poll_fn(move |cx| task(id, generation, cx)));
}

/// A poll of the task of entry `id`'s future, in generation `generation`:
/// done once the future is.
fn task(id: usize, generation: u32, cx: &mut Context) -> Poll<()> {
    let serve = with(|w| {
        let e = &mut w.conns[id];
        if e.generation != generation {
            return None;
        }
        let serve = e.serve.take()?;
        if !e.task.as_ref().is_some_and(|t| t.will_wake(cx.waker())) {
            e.task = Some(cx.waker().clone());
        }
        Some(serve)
    });
    let Some(serve) = serve else {
        return Poll::Ready(());
    };
    match step(serve, cx) {
        Some(serve) => {
            with(|w| w.conns[id].serve = Some(serve));
            Poll::Pending
        }
        None => {
            with(|w| w.finish(id));
            Poll::Ready(())
        }
    }
}

/// Polls the future of the connection `token` names on the driver, with
/// its task's waker: what the task would do if woken, without waking it.
fn inline(token: u64) {
    let (id, generation) = (token as u32 as usize, (token >> 32) as u32);
    let taken = with(|w| {
        let e = &mut w.conns[id];
        if e.generation != generation || e.task.is_none() {
            return None;
        }
        let serve = e.serve.take()?;
        Some((serve, e.task.take()?))
    });
    let Some((serve, waker)) = taken else {
        return;
    };
    #[cfg(test)]
    tests::INLINED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    match step(serve, &mut Context::from_waker(&waker)) {
        Some(serve) => with(|w| {
            let e = &mut w.conns[id];
            (e.serve, e.task) = (Some(serve), Some(waker));
        }),
        None => {
            with(|w| w.finish(id));
            waker.wake(); // its task ends
        }
    }
}

/// A connection on its worker's epoll. Like the future that holds it, it
/// stays on the worker's thread.
pub(crate) struct Sock {
    id: usize,
    stream: Option<TcpStream>,
}

impl Sock {
    pub(crate) fn new(stream: TcpStream) -> Sock {
        let id = with(|w| w.open(stream.as_raw_fd()));
        Sock {
            id,
            stream: Some(stream),
        }
    }

    /// Appends what came to `buf`: `Ok(0)` once the peer has closed. Safe to
    /// drop unfinished.
    pub(crate) async fn read(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        self.read_by(buf, 0).await
    }

    /// `read`, failing with `TimedOut` if nothing came by `deadline` (in
    /// `http::seconds()`; 0 for never). The driver keeps the deadline: no
    /// timer a connection.
    pub(crate) async fn read_by(&mut self, buf: &mut Vec<u8>, deadline: u64) -> io::Result<usize> {
        poll_fn(|cx| with(|w| w.read(self.id, buf, deadline, cx))).await
    }

    /// Sends all of `buf` and leaves it empty. What the socket has no room
    /// for goes on without the caller; a failure then shows in the next call.
    pub(crate) async fn write(&mut self, buf: &mut Vec<u8>) -> io::Result<()> {
        self.flush().await?;
        with(|w| w.write(self.id, buf))
    }

    /// Waits for the send under way.
    async fn flush(&self) -> io::Result<()> {
        poll_fn(|cx| with(|w| w.flushed(self.id, cx))).await
    }

    /// Ends the sending side, once all of it has been sent.
    pub(crate) async fn shutdown(&mut self) {
        if self.flush().await.is_ok()
            && let Some(s) = &self.stream
        {
            let _ = s.shutdown(Shutdown::Write);
        }
    }

    /// The socket as tokio's, once all of it has been sent, and `early`:
    /// what was received and not read is still in the socket.
    pub(crate) async fn into_tcp(
        mut self,
        early: Vec<u8>,
    ) -> io::Result<(tokio::net::TcpStream, Vec<u8>)> {
        self.flush().await?;
        let stream = self.stream.take().ok_or(io::ErrorKind::NotConnected)?;
        with(|w| ctl(w.epoll, libc::EPOLL_CTL_DEL, stream.as_raw_fd(), 0, 0))?;
        Ok((tokio::net::TcpStream::from_std(stream)?, early))
    }
}

impl Drop for Sock {
    fn drop(&mut self) {
        let _ = WORKER.try_with(|w| {
            if let Ok(mut w) = w.try_borrow_mut()
                && let Some(w) = w.as_mut()
            {
                w.close(self.id, self.stream.take());
            }
        });
    }
}

/// The next connection waiting in `listener`, non-blocking like it.
fn accept(listener: &TcpListener) -> io::Result<TcpStream> {
    let flags = libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC;
    // SAFETY: accept4(2) without the peer's address, so no pointers.
    let fd = unsafe {
        libc::accept4(
            listener.as_raw_fd(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            flags,
        )
    };
    owned(fd).map(TcpStream::from)
}

/// A worker's driver: accepts on `listener` (from `uring::listen`), gives
/// each connection to `accepted` (which `spawn`s it), turns the epoll
/// whenever tokio's says it has events, and polls the connections those
/// events are for. Stops accepting once the server is stopping; runs until
/// the process ends.
pub(crate) async fn serve(listener: TcpListener, accepted: fn(TcpStream)) {
    // SAFETY: epoll_create1(2), no pointers.
    let epoll = owned(unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) }).and_then(|epoll| {
        listener.set_nonblocking(true)?;
        let events = (libc::EPOLLIN | libc::EPOLLET) as u32;
        let l = listener.as_raw_fd();
        ctl(epoll.as_raw_fd(), libc::EPOLL_CTL_ADD, l, events, ACCEPT)?;
        let raw = epoll.as_raw_fd();
        Ok((AsyncFd::with_interest(epoll, Interest::READABLE)?, raw))
    });
    let (epoll, raw) = match epoll {
        Ok(e) => e,
        Err(e) => crate::fail(&format!("epoll cannot be set up: {e}")),
    };
    WORKER.set(Some(Worker {
        epoll: raw,
        conns: Vec::new(),
        free: Vec::new(),
        events: Vec::with_capacity(EVENTS),
        ready: Vec::new(),
        checked: 0,
        acceptable: true,
    }));
    let mut listener = Some(listener);
    let mut stop = std::pin::pin!(http::stopped());
    // Wakes the driver for `tick` when nothing else does.
    let mut tick = std::pin::pin!(tokio::time::sleep(Duration::from_secs(1)));
    // Accepting again after a failure (out of descriptors) waits until then.
    let mut retry: Option<Instant> = None;
    let mut wait = std::pin::pin!(tokio::time::sleep(Duration::ZERO));
    let mut acceptable = false;
    // The `ready` of the last turn, swapped with the worker's: no allocation.
    let mut ready = Vec::new();
    loop {
        if retry.is_some_and(|t| t <= Instant::now()) {
            retry = None;
        }
        let full = with(|w| {
            let full = w.turn();
            acceptable |= std::mem::take(&mut w.acceptable);
            std::mem::swap(&mut ready, &mut w.ready);
            full
        });
        for &token in &ready {
            inline(token);
        }
        ready.clear();
        // Edge-triggered: all that waits, or the listener says nothing more.
        while let Some(l) = &listener
            && acceptable
            && retry.is_none()
        {
            match accept(l) {
                Ok(s) => accepted(s),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => acceptable = false,
                Err(e) => {
                    if http::accept_failed(&e) {
                        retry = Some(Instant::now() + Duration::from_millis(50));
                    }
                }
            }
        }
        if full {
            // More events wait: the tasks woken go first, so a busy epoll
            // never starves the worker's other tasks.
            yield_once().await;
            continue;
        }
        poll_fn(|cx| {
            if listener.is_some() && stop.as_mut().poll(cx).is_ready() {
                // Refuses new connections at once.
                listener = None;
                retry = None;
                return Poll::Ready(());
            }
            if let Poll::Ready(ready) = epoll.poll_read_ready(cx) {
                // Cleared before the turn, so what comes during it wakes us again.
                if let Ok(mut guard) = ready {
                    guard.clear_ready();
                }
                return Poll::Ready(());
            }
            // Deadlines and stalled sends are checked once a second while
            // there are connections, without anything else to wake the driver.
            if with(|w| w.conns.len() > w.free.len()) && tick.as_mut().poll(cx).is_ready() {
                tick.as_mut().reset(Instant::now() + Duration::from_secs(1));
                return Poll::Ready(());
            }
            if let Some(due) = retry {
                if wait.deadline() != due {
                    wait.as_mut().reset(due);
                }
                if wait.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(());
                }
            }
            Poll::Pending
        })
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uring::listen;
    use std::io::{Read, Write};
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Futures the driver polled itself, in every test's workers.
    pub(super) static INLINED: AtomicUsize = AtomicUsize::new(0);

    /// A worker serving each connection with `each`.
    fn server(each: fn(TcpStream)) -> SocketAddr {
        let listener = listen("127.0.0.1:0".parse().unwrap()).unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                tokio::spawn(serve(listener, each));
                std::future::pending::<()>().await
            });
        });
        addr
    }

    fn echo(s: TcpStream) {
        let mut sock = Sock::new(s);
        tokio::spawn(async move {
            let mut buf = Vec::new();
            while let Ok(n) = sock.read(&mut buf).await
                && n > 0
                && sock.write(&mut buf).await.is_ok()
            {}
            sock.shutdown().await;
        });
    }

    fn pattern(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i % 251) as u8).collect()
    }

    fn connect(addr: SocketAddr) -> std::net::TcpStream {
        let c = std::net::TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        c
    }

    #[test]
    fn echoes_megabytes_in_order() {
        // More than the socket buffers hold at once: partial sends finished
        // by the driver, receives that wait for them.
        let addr = server(echo);
        let sent = pattern(8 << 20);
        let mut c = connect(addr);
        let mut w = c.try_clone().unwrap();
        let data = sent.clone();
        let writer = std::thread::spawn(move || {
            w.write_all(&data).unwrap();
            w.shutdown(Shutdown::Write).unwrap();
        });
        let mut got = Vec::new();
        c.read_to_end(&mut got).unwrap();
        writer.join().unwrap();
        assert!(got == sent, "{} bytes back of {}", got.len(), sent.len());
    }

    #[test]
    fn many_connections_at_once() {
        // More than one turn's events (256), within the default 1024 descriptors.
        let addr = server(echo);
        let mut conns: Vec<_> = (0..300).map(|_| connect(addr)).collect();
        for round in 0..3u8 {
            for (i, c) in conns.iter_mut().enumerate() {
                c.write_all(format!("{round}:{i};").as_bytes()).unwrap();
            }
            for (i, c) in conns.iter_mut().enumerate() {
                let want = format!("{round}:{i};");
                let mut got = vec![0; want.len()];
                c.read_exact(&mut got).unwrap();
                assert_eq!(got, want.as_bytes());
            }
        }
    }

    /// Answers each line, in order, as `http::requests` answers requests:
    /// `wait` after a timer (a handler that awaits), `big` with 4 MB (a send
    /// the socket has no room for), `panic` not at all, others as they are.
    fn lines(s: TcpStream) {
        spawn(s, |mut sock| async move {
            let (mut buf, mut out) = (Vec::new(), Vec::new());
            loop {
                while let Some(end) = buf.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = buf.drain(..=end).collect();
                    match &line[..] {
                        b"wait\n" => {
                            tokio::time::sleep(Duration::from_millis(20)).await;
                            out.extend_from_slice(b"waited\n");
                        }
                        b"big\n" => out.extend_from_slice(&pattern(4 << 20)),
                        b"panic\n" => panic!("a test panic"),
                        _ => out.extend_from_slice(&line),
                    }
                    if sock.write(&mut out).await.is_err() {
                        return;
                    }
                }
                if !sock.read(&mut buf).await.is_ok_and(|n| n > 0) {
                    return;
                }
            }
        });
    }

    fn ask(c: &mut std::net::TcpStream, line: &str, want: &[u8]) {
        c.write_all(line.as_bytes()).unwrap();
        let mut got = vec![0; want.len()];
        c.read_exact(&mut got).unwrap();
        assert!(got == want, "{line:?}");
    }

    #[test]
    fn requests_after_the_first_are_answered_by_the_driver() {
        // The first goes through the task, and so does one that awaits; the
        // ones after each are polled by the driver.
        let addr = server(lines);
        let mut c = connect(addr);
        let before = INLINED.load(Ordering::Relaxed);
        ask(&mut c, "a\n", b"a\n");
        ask(&mut c, "wait\n", b"waited\n");
        for i in 0..20 {
            ask(&mut c, &format!("x{i}\n"), format!("x{i}\n").as_bytes());
        }
        let inlined = INLINED.load(Ordering::Relaxed) - before;
        assert!(inlined >= 21, "{inlined} polled by the driver");
    }

    #[test]
    fn pipelined_requests_mix_the_driver_and_the_task() {
        // Answers that are ready, that await, and that the socket has no
        // room for, in one read: all in order, and after them the driver
        // answers again.
        let addr = server(lines);
        let mut c = connect(addr);
        ask(&mut c, "a\n", b"a\n");
        let mut want = b"b\nwaited\n".to_vec();
        want.extend_from_slice(&pattern(4 << 20));
        want.extend_from_slice(b"c\nwaited\nd\n");
        ask(&mut c, "b\nwait\nbig\nc\nwait\nd\n", &want);
        let before = INLINED.load(Ordering::Relaxed);
        ask(&mut c, "e\n", b"e\n");
        assert!(INLINED.load(Ordering::Relaxed) > before);
    }

    #[test]
    fn a_panic_ends_its_connection_not_the_driver() {
        let addr = server(lines);
        let mut c = connect(addr);
        ask(&mut c, "a\n", b"a\n");
        c.write_all(b"panic\n").unwrap();
        let mut rest = Vec::new();
        let _ = c.read_to_end(&mut rest);
        assert!(rest.is_empty());
        let mut next = connect(addr);
        ask(&mut next, "b\n", b"b\n");
        ask(&mut next, "c\n", b"c\n");
    }

    #[test]
    fn a_read_past_its_deadline_fails() {
        fn waits(s: TcpStream) {
            spawn(s, |mut sock| async move {
                let mut buf = Vec::new();
                let deadline = http::seconds() + 1;
                if let Err(e) = sock.read_by(&mut buf, deadline).await {
                    let _ = sock
                        .write(&mut format!("{:?}", e.kind()).into_bytes())
                        .await;
                }
            });
        }
        http::start_clock();
        let addr = server(waits);
        let mut c = connect(addr);
        let mut got = String::new();
        c.read_to_string(&mut got).unwrap();
        assert_eq!(got, "TimedOut");
    }

    #[test]
    fn a_response_outlives_its_socket() {
        // Sent in part, then the socket dropped at once: all of it still
        // arrives, then the end.
        fn answer(s: TcpStream) {
            let mut sock = Sock::new(s);
            tokio::spawn(async move {
                let mut buf = pattern(4 << 20);
                let _ = sock.write(&mut buf).await;
            });
        }
        let addr = server(answer);
        let mut c = connect(addr);
        std::thread::sleep(Duration::from_millis(200));
        let mut got = Vec::new();
        c.read_to_end(&mut got).unwrap();
        assert!(got == pattern(4 << 20), "{} bytes", got.len());
    }

    #[test]
    fn a_socket_goes_to_tokio_with_what_came() {
        fn handover(s: TcpStream) {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut sock = Sock::new(s);
            tokio::spawn(async move {
                let mut buf = Vec::new();
                while buf.len() < 5 {
                    sock.read(&mut buf).await.unwrap();
                }
                sock.write(&mut b"ok".to_vec()).await.unwrap();
                // What comes now waits in the socket.
                tokio::time::sleep(Duration::from_millis(100)).await;
                let (mut tcp, mut early) = sock.into_tcp(buf.split_off(5)).await.unwrap();
                while early.len() < 6 {
                    tcp.read_buf(&mut early).await.unwrap();
                }
                tcp.write_all(&early).await.unwrap();
            });
        }
        let addr = server(handover);
        let mut c = connect(addr);
        c.write_all(b"hello").unwrap();
        let mut ok = [0; 2];
        c.read_exact(&mut ok).unwrap();
        assert_eq!(&ok, b"ok");
        c.write_all(b"world!").unwrap();
        let mut back = [0; 6];
        c.read_exact(&mut back).unwrap();
        assert_eq!(&back, b"world!");
    }

    #[test]
    fn a_closed_peer_reads_as_the_end() {
        fn ends(s: TcpStream) {
            let mut sock = Sock::new(s);
            tokio::spawn(async move {
                let mut buf = Vec::new();
                while sock.read(&mut buf).await.is_ok_and(|n| n > 0) {}
                let mut said = format!("{} bytes", buf.len()).into_bytes();
                let _ = sock.write(&mut said).await;
            });
        }
        let addr = server(ends);
        let mut c = connect(addr);
        c.write_all(b"abc").unwrap();
        c.shutdown(Shutdown::Write).unwrap();
        let mut got = String::new();
        c.read_to_string(&mut got).unwrap();
        assert_eq!(got, "3 bytes");
    }

    #[test]
    fn an_end_that_came_with_the_bytes_is_read() {
        // The bytes and the end arrive together, and the driver takes their
        // one event before the first read: the read after the bytes gets the
        // end, with no event left to say so.
        fn late(s: TcpStream) {
            let mut sock = Sock::new(s);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(200)).await;
                let mut buf = Vec::new();
                while sock.read(&mut buf).await.is_ok_and(|n| n > 0) {}
                let mut said = format!("{} bytes", buf.len()).into_bytes();
                let _ = sock.write(&mut said).await;
            });
        }
        let addr = server(late);
        let mut c = connect(addr);
        c.write_all(b"GET /hel").unwrap();
        c.shutdown(Shutdown::Write).unwrap();
        let mut got = String::new();
        c.read_to_string(&mut got).unwrap();
        assert_eq!(got, "8 bytes");
    }
}
