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
//! Between requests the future holds nothing (`readable`), and the driver
//! goes one better: it answers requests to the routes the build found never
//! wait itself (`http::on_driver`), without polling the future at all. The
//! first request it cannot answer, and the rest after it, it hands to the
//! future.
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

use crate::driver::{self, Driver, Slab, Slot, result, token, untoken, wait};
use crate::uring::{owned, yield_once};
use crate::{http, policy};
use std::cell::RefCell;
use std::future::{Future, poll_fn};
use std::io;
use std::net::{SocketAddr, TcpListener, TcpStream};
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

/// File descriptors kept back from connections when the cap comes from
/// the limit: listeners, epolls and rings, files, database sockets.
const FD_RESERVE: u64 = 1024;
/// Memory a held connection takes at most, measured with 16384 idle
/// keep-alive connections; the cap leaves them a quarter of the memory.
const CONN_BYTES: u64 = 16 * 1024;

/// The connection cap when `WISP_MAX_CONNS` is not set, read once at start:
/// the process's open-file limit (its soft one raised to the hard one, as
/// far as the system allows), less `FD_RESERVE`, and no more than a quarter
/// of the memory holds. Past it a new connection is answered 503, where one
/// more socket would fail its accept or files would fail to open.
pub(crate) fn default_max_conns() -> usize {
    let mut lim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: an rlimit alive for the call, which the kernel writes.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut lim) } != 0 {
        return 10_000;
    }
    if lim.rlim_cur < lim.rlim_max {
        let up = libc::rlimit {
            rlim_cur: lim.rlim_max,
            rlim_max: lim.rlim_max,
        };
        // SAFETY: an rlimit alive for the call, which the kernel only reads.
        if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raw const up) } == 0 {
            lim = up;
        }
    }
    let mem = std::fs::read_to_string("/proc/meminfo").ok().and_then(|m| {
        let kb = m.lines().find_map(|l| l.strip_prefix("MemTotal:"))?;
        kb.trim().trim_end_matches("kB").trim().parse::<u64>().ok()
    });
    let cap = conns_for(lim.rlim_cur, mem.map(|kb| kb * 1024));
    if cap < 16384 {
        http::log(format_args!(
            "wisp: {cap} connections at most (open-file limit {}); past them new ones answer 503
  Raise `ulimit -n` (LimitNOFILE= under systemd) or set WISP_MAX_CONNS.",
            lim.rlim_cur
        ));
    }
    cap
}

/// The cap `fds` open files and `mem` bytes of memory allow: half the files
/// when there are few, else all but `FD_RESERVE`, and a quarter of `mem` at
/// `CONN_BYTES` each; never under 64 or over 2^20.
fn conns_for(fds: u64, mem: Option<u64>) -> usize {
    let by_fds = if fds <= 2 * FD_RESERVE {
        fds / 2
    } else {
        fds - FD_RESERVE
    };
    let by_mem = mem.map_or(u64::MAX, |m| m / 4 / CONN_BYTES);
    by_fds.min(by_mem).clamp(64, 1 << 20) as usize
}

/// Receives onto the end of `buf`, into the room it has: the bytes, 0 at
/// the end of the stream, or the errno. The system calls here are made
/// straight, as `recv(2)` and `send(2)` make them but without libc's
/// thread-cancellation bookkeeping around each, which Wisp never uses.
fn recv(fd: RawFd, buf: &mut Vec<u8>) -> Result<usize, i32> {
    let room = buf.spare_capacity_mut();
    let (at, len) = (room.as_mut_ptr(), room.len());
    let none = std::ptr::null_mut::<libc::c_void>();
    loop {
        // SAFETY: recvfrom(2) writes at most `len` bytes at `at`, the
        // vector's room, which nothing else refers to during the call; no
        // address is asked for.
        let n = unsafe { libc::syscall(libc::SYS_recvfrom, fd, at, len, 0, none, none) } as isize;
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
        // SAFETY: sendto(2) reads `buf`, alive for the call, to no address.
        // MSG_NOSIGNAL: a peer gone is EPIPE, not SIGPIPE.
        let (at, flags) = (buf.as_ptr(), libc::MSG_NOSIGNAL);
        let none = std::ptr::null::<libc::c_void>();
        let n =
            unsafe { libc::syscall(libc::SYS_sendto, fd, at, buf.len(), flags, none, 0) } as isize;
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
    /// The rest of a send the socket had no room for, from `sent` on.
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
    /// The future waits in `readable`, holding nothing: the driver may
    /// answer what comes itself (`http::on_driver`).
    bare: bool,
    /// The peer, for the requests the driver answers: set with `serve`.
    peer: Option<SocketAddr>,
    /// What the driver received and leaves to the future (`http::Handed`).
    handed: Option<http::Handed>,
    /// That future, while nothing polls it.
    serve: Option<Serve>,
    /// The waker of its task, which the driver polls it with.
    task: Option<Waker>,
}

impl Slot for Entry {
    fn generation(&self) -> u32 {
        self.generation
    }
}

impl Entry {
    fn wake(&mut self) {
        (self.wants_recv, self.wants_send, self.bare) = (false, false, false);
        if let Some(w) = &self.waker {
            w.wake_by_ref();
        }
    }
}

/// A worker's epoll and connections.
pub(crate) struct Worker {
    epoll: RawFd,
    conns: Slab<Entry>,
    events: Vec<libc::epoll_event>,
    /// The connections (`token`s) whose futures the driver polls after the
    /// turn: their sockets have what they wait for.
    ready: Vec<u64>,
    /// The second deadlines and sends were last checked (`tick`).
    checked: u64,
    /// The listener had an event: connections may wait in it.
    acceptable: bool,
    /// The driver answers requests itself (`serve`'s `now`).
    answers: bool,
    /// The connections (`token`s) with something to receive whose futures
    /// wait holding nothing: the driver tries them first.
    fast: Vec<u64>,
    /// Buffers of sends that are over, for the next sends the socket has no
    /// room for to swap in: a connection keeps none once its send is done.
    spare: Vec<Vec<u8>>,
}

/// How many buffers `spare` keeps.
const SPARE: usize = 64;

thread_local! {
    /// This thread's worker, set once its driver starts.
    static WORKER: RefCell<Option<Worker>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Worker) -> R) -> R {
    driver::with(f)
}

impl Worker {
    /// One `epoll_wait`, without waiting: hands out the events that came.
    /// Whether any did (or a signal cut it short): another turn may find more.
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
        n > 0
    }

    fn event(&mut self, data: u64, events: u32) {
        if data == ACCEPT {
            self.acceptable = true;
            return;
        }
        let (id, generation) = untoken(data);
        let e = &mut self.conns[id];
        if e.fd < 0 || e.generation != generation {
            return;
        }
        if events & READABLE != 0 {
            e.readable = true;
            e.ended |= events & ENDED != 0;
            if self.answers && e.bare && e.wants_recv && !e.ended {
                self.fast.push(data);
            } else if e.wants_recv {
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
        let mut out = std::mem::take(&mut e.out);
        out.clear();
        if policy::kept(out.capacity()) && self.spare.len() < SPARE {
            self.spare.push(out);
        }
        http::send_under_way(false);
        let e = &mut self.conns[id];
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
            if e.sending && policy::stalled(now, e.since) {
                self.sent(id, libc::ETIMEDOUT);
            } else if e.wants_recv && e.deadline != 0 && now >= e.deadline {
                e.wake();
            }
        }
    }

    /// Entry `id` is done with its socket, which is closed if it still has
    /// it; free unless its future still runs.
    fn release(&mut self, id: usize) {
        let e = &mut self.conns[id];
        (e.fd, e.close, e.waker) = (-1, None, None);
        if !e.attached {
            self.conns.free(id);
        }
    }

    /// Entry `id`'s future ended: free unless its socket is still in use.
    fn finish(&mut self, id: usize) {
        let e = &mut self.conns[id];
        (e.attached, e.serve, e.task, e.handed) = (false, None, None, None);
        if e.fd < 0 {
            self.conns.free(id);
        }
    }

    /// Receives onto the end of `buf` what the socket may have: the bytes,
    /// 0 at its end, or `WouldBlock` when it has nothing until its next
    /// event.
    fn try_read(&mut self, id: usize, buf: &mut Vec<u8>) -> io::Result<usize> {
        let e = &mut self.conns[id];
        result(e.failed)?;
        if !e.readable {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        buf.reserve(RECV_ROOM);
        let room = buf.capacity() - buf.len();
        match recv(e.fd, buf) {
            Ok(n) => {
                // Less than it could take: the socket is empty until its
                // next event, unless its end came with what was read.
                e.readable = e.ended || n == room || n == 0;
                Ok(n)
            }
            Err(libc::EAGAIN) => {
                e.readable = false;
                Err(io::ErrorKind::WouldBlock.into())
            }
            Err(errno) => Err(io::Error::from_raw_os_error(errno)),
        }
    }

    fn read(
        &mut self,
        id: usize,
        buf: &mut Vec<u8>,
        deadline: u64,
        cx: &Context,
    ) -> Poll<io::Result<usize>> {
        match self.try_read(id, buf) {
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            done => return Poll::Ready(done),
        }
        let e = &mut self.conns[id];
        if deadline != 0 && http::seconds() >= deadline {
            return Poll::Ready(Err(io::ErrorKind::TimedOut.into()));
        }
        (e.wants_recv, e.deadline) = (true, deadline);
        wait(&mut e.waker, cx);
        Poll::Pending
    }

    /// Ready once the socket may have something to receive (or its end, or
    /// an error, which the receive after it reports), as `read` waits but
    /// with no buffer, until the entry's deadline: the one the future set
    /// as it began to wait, or a later one the driver set as it answered
    /// requests itself (`next`).
    fn readable(&mut self, id: usize, cx: &Context) -> Poll<io::Result<()>> {
        let e = &mut self.conns[id];
        result(e.failed)?;
        if e.readable || e.handed.is_some() {
            return Poll::Ready(Ok(()));
        }
        if e.deadline != 0 && http::seconds() >= e.deadline {
            return Poll::Ready(Err(io::ErrorKind::TimedOut.into()));
        }
        (e.wants_recv, e.bare) = (true, true);
        wait(&mut e.waker, cx);
        Poll::Pending
    }

    /// Receives onto the end of `buf`, as `read` does but without waiting.
    /// Not `try_read`'s: its checks, which the driver made already, cost
    /// the request on the driver more than this does.
    fn receive(&mut self, id: usize, buf: &mut Vec<u8>) -> Got {
        let e = &mut self.conns[id];
        buf.reserve(RECV_ROOM);
        let room = buf.capacity() - buf.len();
        match recv(e.fd, buf) {
            Ok(0) => Got::End, // and every receive after it
            Ok(n) => {
                e.readable = e.ended || n == room;
                Got::Bytes(e.readable)
            }
            Err(libc::EAGAIN) => {
                e.readable = false;
                Got::Nothing
            }
            Err(errno) => {
                e.failed = errno;
                Got::End
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
    peer: SocketAddr,
    serve: impl FnOnce(Sock) -> F,
) {
    let sock = Sock::new(stream);
    let id = sock.id;
    let serve: Serve = Box::pin(serve(sock));
    let generation = with(|w| {
        let e = &mut w.conns[id];
        (e.attached, e.serve, e.peer) = (true, Some(serve), Some(peer));
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
    let (id, generation) = untoken(token);
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

/// The future of connection `token` takes over from the driver, which
/// tried its requests first (`http::on_driver`): polled as an event would have.
fn resume(token: u64) {
    let (id, generation) = untoken(token);
    let polled = with(|w| {
        let e = &mut w.conns[id];
        if e.generation != generation || !e.wants_recv {
            return false;
        }
        if e.task.is_some() {
            (e.wants_recv, e.bare) = (false, false);
            return true;
        }
        e.wake();
        false
    });
    if polled {
        inline(token);
    }
}

/// What `receive` got.
pub(crate) enum Got {
    /// Bytes, and whether the socket may have more.
    Bytes(bool),
    /// Nothing yet.
    Nothing,
    /// The peer's end or an error, which the future then reads.
    End,
}

/// For `http::on_driver`: the entry and peer of connection `token`, when
/// its future waits holding nothing and the socket is the driver's to
/// receive from (`receive`) and send on.
pub(crate) fn free(token: u64) -> Option<(usize, SocketAddr)> {
    let (id, generation) = untoken(token);
    with(|w| {
        let e = &w.conns[id];
        let free = e.fd >= 0 && e.generation == generation && e.bare && e.wants_recv;
        if !free || e.ended || e.sending || e.failed != 0 || e.handed.is_some() {
            return None;
        }
        Some((id, e.peer?))
    })
}

/// For `http::on_driver`: what a receive onto the end of `buf` got for
/// connection `id`, which `free` gave it.
pub(crate) fn receive(id: usize, buf: &mut Vec<u8>) -> Got {
    with(|w| w.receive(id, buf))
}

/// For `http::on_driver`, once it answered what connection `id` sent: sends
/// `out` as `Sock::write` does, and the connection waits for its next
/// request until `deadline`. Then, if all of it went and the socket may
/// have `more`, receives onto `buf`. `End` when the future goes on from
/// here: the send failed, or the rest goes as the socket takes it.
pub(crate) fn next(
    id: usize,
    out: &mut Vec<u8>,
    buf: &mut Vec<u8>,
    more: bool,
    deadline: u64,
) -> Got {
    with(|w| {
        w.conns[id].deadline = deadline;
        if !out.is_empty() && (w.write(id, out).is_err() || w.conns[id].sending) {
            return Got::End;
        }
        if !more {
            return Got::Nothing;
        }
        w.receive(id, buf)
    })
}

/// Sends what `h` has to send, as `Sock::write` does, and leaves the rest
/// of it to the future of connection `id`, whose `readable` is then ready.
/// A failure shows in the future's next call.
pub(crate) fn hand(id: usize, mut h: http::Handed) {
    with(|w| {
        if let Some(out) = h.wbuf()
            && !out.is_empty()
        {
            let _ = w.write(id, out);
        }
        w.conns[id].handed = Some(h);
    });
}

impl Driver for Worker {
    fn local() -> &'static std::thread::LocalKey<RefCell<Option<Worker>>> {
        &WORKER
    }
    const OFF: &'static str = "an epoll socket used off its worker";

    /// An entry for the socket `fd`, which its events reach from now on.
    fn open(&mut self, fd: RawFd) -> usize {
        let (id, generation) = self.conns.open();
        let e = &mut self.conns[id];
        *e = Entry {
            fd,
            generation,
            // Its request is likely in already: the first read tries.
            readable: true,
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

    /// Sends all of `buf`, and leaves it empty: what the socket has no room
    /// for goes on from the driver (and `buf` gets a spare buffer).
    fn write(&mut self, id: usize, buf: &mut Vec<u8>) -> io::Result<()> {
        let Worker { conns, spare, .. } = self;
        let e = &mut conns[id];
        debug_assert!(!e.sending && e.out.is_empty());
        result(e.failed)?;
        let mut at = 0;
        while at < buf.len() {
            match send(e.fd, &buf[at..]) {
                Ok(n) => at += n,
                Err(libc::EAGAIN) => {
                    e.out = std::mem::replace(buf, spare.pop().unwrap_or_default());
                    (e.sent, e.sending, e.since) = (at, true, http::seconds());
                    http::send_under_way(true);
                    return Ok(());
                }
                Err(errno) => {
                    e.failed = errno;
                    buf.clear();
                    return result(e.failed);
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
            wait(&mut e.waker, cx);
            return Poll::Pending;
        }
        Poll::Ready(result(e.failed))
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

/// Connection `token` fails: every call of its future does.
fn fail(token: u64) {
    let (id, generation) = untoken(token);
    with(|w| {
        let e = &mut w.conns[id];
        if e.generation == generation && e.failed == 0 {
            e.failed = libc::EIO;
        }
    });
}

/// A connection on its worker's epoll.
pub(crate) type Sock = driver::Sock<Worker>;

impl Sock {
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

    /// `read`, but `WouldBlock` rather than waiting.
    pub(crate) fn try_read(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        with(|w| w.try_read(self.id, buf))
    }

    /// Waits, with no buffer, until a `read_by` would not: the socket may
    /// have something to receive. `TimedOut` past `deadline`, as there,
    /// or past a later one the driver set as it answered requests itself:
    /// a future that waits again after a wakeup with nothing behind it
    /// keeps that.
    pub(crate) async fn readable_by(&mut self, deadline: u64) -> io::Result<()> {
        with(|w| {
            let e = &mut w.conns[self.id];
            e.deadline = e.deadline.max(deadline);
        });
        poll_fn(|cx| with(|w| w.readable(self.id, cx))).await
    }

    /// What the driver left to this connection's future, once `readable`.
    pub(crate) fn handed(&mut self) -> Option<http::Handed> {
        with(|w| w.conns[self.id].handed.take())
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
/// events are for. `now` (`http::on_driver`) first answers what it can of a
/// connection whose future waits holding nothing, and says whether the
/// future has anything to do. Says on `ready` once it is set up. Stops
/// accepting once the server is stopping; runs until the process ends.
pub(crate) async fn serve(
    listener: TcpListener,
    accepted: fn(TcpStream),
    now: Option<fn(u64) -> bool>,
    ready: std::sync::mpsc::Sender<()>,
) {
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
        conns: Slab::new(),
        events: Vec::with_capacity(EVENTS),
        ready: Vec::new(),
        checked: 0,
        acceptable: true,
        answers: now.is_some(),
        fast: Vec::new(),
        spare: Vec::new(),
    }));
    let _ = ready.send(());
    let mut listener = Some(listener);
    let mut stop = std::pin::pin!(http::stopped());
    // Wakes the driver for `tick` when nothing else does.
    let mut tick = std::pin::pin!(tokio::time::sleep(Duration::from_secs(1)));
    // Accepting again after a failure (out of descriptors) waits until then.
    let mut retry: Option<Instant> = None;
    let mut wait = std::pin::pin!(tokio::time::sleep(Duration::ZERO));
    let mut acceptable = false;
    // The `ready` and `fast` of the last turn, swapped with the worker's: no
    // allocation.
    let (mut ready, mut fast) = (Vec::new(), Vec::new());
    loop {
        if retry.is_some_and(|t| t <= Instant::now()) {
            retry = None;
        }
        if listener.is_some() && http::stopping() {
            (listener, retry) = (None, None); // refuses new connections at once
        }
        let took = with(|w| {
            let took = w.turn();
            acceptable |= std::mem::take(&mut w.acceptable);
            std::mem::swap(&mut ready, &mut w.ready);
            std::mem::swap(&mut fast, &mut w.fast);
            took
        });
        if let Some(now) = now {
            for &token in &fast {
                // A panic past the handlers' own `catch` is a bug in Wisp:
                // the connection fails, the driver goes on.
                match catch_unwind(|| now(token)) {
                    Ok(true) => {}
                    Ok(false) => resume(token),
                    Err(_) => {
                        fail(token);
                        resume(token);
                    }
                }
            }
        }
        fast.clear();
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
        if took {
            // Events came, so more likely wait: the next turn is ours again
            // after the tasks woken, so a busy epoll never starves them, and
            // tokio turns its own epoll (which waits on ours) only now and
            // then rather than once a turn. A turn that takes none waits.
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
            if with(|w| w.conns.busy()) && tick.as_mut().poll(cx).is_ready() {
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
    use crate::uring::{listen, socket_tests};
    use std::io::{Read, Write};
    use std::net::Shutdown;

    #[test]
    fn the_default_cap_follows_the_fd_limit_and_memory() {
        // 16384 clients on a host with the usual 1048576 hard limit and
        // 4 GiB are all held, where the old fixed 10000 refused them.
        assert!(conns_for(1 << 20, Some(4 << 30)) >= 16384);
        assert_eq!(conns_for(1 << 20, None), (1 << 20) - 1024);
        assert_eq!(conns_for(65536, None), 65536 - 1024);
        assert_eq!(conns_for(1024, None), 512); // a low limit leaves half
        assert_eq!(
            conns_for(1 << 20, Some(1 << 30)),
            (1 << 30) / 4 / CONN_BYTES as usize
        );
        assert_eq!(conns_for(10, None), 64);
        assert_eq!(conns_for(u64::MAX, None), 1 << 20);
        // At start the soft limit is raised to the hard one, and the cap
        // stays under it, so a held connection never fails for a descriptor.
        let cap = default_max_conns();
        let mut lim = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: an rlimit alive for the call, which the kernel writes.
        assert_eq!(
            unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut lim) },
            0
        );
        assert_eq!(lim.rlim_cur, lim.rlim_max);
        assert!(
            cap as u64 <= lim.rlim_cur.max(128),
            "{cap} over {}",
            lim.rlim_cur
        );
    }
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
                tokio::spawn(serve(listener, each, None, std::sync::mpsc::channel().0));
                std::future::pending::<()>().await
            });
        });
        addr
    }

    socket_tests!(|each| Some(server(each)));

    /// Answers each line, in order, as `http::requests` answers requests:
    /// `wait` after a timer (a handler that awaits), `big` with 4 MB (a send
    /// the socket has no room for), `panic` not at all, others as they are.
    fn lines(s: TcpStream) {
        spawn(s, ([127, 0, 0, 1], 0).into(), |mut sock| async move {
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
            spawn(s, ([127, 0, 0, 1], 0).into(), |mut sock| async move {
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
    fn a_deadline_the_driver_moved_on_holds() {
        // The driver answered requests itself meanwhile (`next`): the
        // deadline the future waited with, now past, does not end it.
        fn waits(s: TcpStream) {
            spawn(s, ([127, 0, 0, 1], 0).into(), |mut sock| async move {
                tokio::time::sleep(Duration::from_millis(1100)).await;
                let _ = sock.try_read(&mut Vec::new()); // nothing yet
                with(|w| w.conns[sock.id].deadline = http::seconds() + 100);
                let said = match sock.readable_by(http::seconds()).await {
                    Ok(()) => "ok".to_string(),
                    Err(e) => format!("{:?}", e.kind()),
                };
                let _ = sock.write(&mut said.into_bytes()).await;
            });
        }
        http::start_clock();
        let addr = server(waits);
        let mut c = connect(addr);
        std::thread::sleep(Duration::from_millis(1500));
        c.write_all(b"x").unwrap();
        let mut got = String::new();
        let _ = c.read_to_string(&mut got);
        assert_eq!(got, "ok");
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
