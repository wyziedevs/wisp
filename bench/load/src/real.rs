//! Many connections on a few threads, for `bench-run --suite real`. Each
//! load thread runs a tokio runtime of its own (current thread), so the
//! runner's pinning of its threads to the load CPUs holds.
//!
//! - [`Users`]: people on keep-alive connections, each sending its next
//!   request at a time of its own whether or not the server kept up (open
//!   loop). Latency runs from that time, so a server that falls behind is
//!   charged for the wait, not just for the requests it got to.
//! - [`churn`]: closed loops opening a connection per request.
//! - [`slow`]: connections sending a request head a byte at a time.
//! - [`closed_loop`]: `run`'s closed loops, for a thousand connections.
//! - [`ws`]: idle WebSockets, some of them echoing.
//!
//! Every connection closes with a reset (linger 0), so growing, shrinking
//! and ending leave no TIME_WAIT behind to run the ports out.

use super::{BLOCK, Histogram, Report, Response, get_request, parse};
use std::cell::{Cell, RefCell};
use std::future::{Future, poll_fn};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::pin::{Pin, pin};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::thread;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpSocket, TcpStream};
use tokio::task::{JoinSet, LocalSet};
use tokio::time::{Sleep, sleep, sleep_until, timeout, timeout_at};

/// How long a request may take before it counts as an error.
const TIMEOUT: Duration = Duration::from_secs(5);
/// How long after a step's window its last requests may still finish.
const GRACE: Duration = Duration::from_secs(1);

/// What bounds a connection's requests by `TIMEOUT` with one timer, not one
/// per request: moved on only when less than `TIMEOUT` of it is left, so a
/// request that stalls fails between `TIMEOUT` and twice that.
struct Deadline(Pin<Box<Sleep>>);

impl Deadline {
    fn new() -> Deadline {
        Deadline(Box::pin(sleep(Duration::ZERO)))
    }

    /// `f`'s answer, or `None` once the deadline passes.
    async fn within<T>(&mut self, f: impl Future<Output = Option<T>>) -> Option<T> {
        let now = tokio::time::Instant::now();
        if self.0.deadline() < now + TIMEOUT {
            self.0.as_mut().reset(now + 2 * TIMEOUT);
        }
        let mut f = pin!(f);
        poll_fn(|cx| match f.as_mut().poll(cx) {
            Poll::Ready(r) => Poll::Ready(r),
            Poll::Pending if self.0.as_mut().poll(cx).is_ready() => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        })
        .await
    }
}

/// The paths a user asks for and how often, in tenths: a page, an API
/// call, a page with a query behind it.
pub const MIX: [(&str, u64); 3] = [("/page", 5), ("/json", 3), ("/fortunes", 2)];

/// The path of `MIX` that a draw in `0..10` picks.
fn pick(mut tenth: u64) -> usize {
    for (i, &(_, weight)) in MIX.iter().enumerate() {
        if tenth < weight {
            return i;
        }
        tenth -= weight;
    }
    MIX.len() - 1
}

/// What one step of users measured: the requests due in its window, and
/// what came of them.
#[derive(Default)]
pub struct Step {
    pub users: usize,
    pub scheduled: u64,
    /// How late the load sent the requests it was waiting to send, in µs:
    /// its own lag, not the server's.
    pub late: Histogram,
    /// What came of them, latency from each request's due time.
    pub r: Report,
}

impl Step {
    /// Whether the server kept up: p99 within `slo_us`, at most 0.1% of the
    /// requests failed, and at least 99% of those due were answered.
    pub fn passes(&self, slo_us: u64) -> bool {
        let r = &self.r;
        let answered = r.ok + r.non_2xx;
        self.scheduled > 0
            && r.latency.percentile(0.99) <= slo_us
            && (r.errors + r.non_2xx) * 1000 <= self.scheduled
            && answered * 100 >= self.scheduled * 99
    }

    fn merge(&mut self, o: &Step) {
        self.scheduled += o.scheduled;
        self.late.merge(&o.late);
        self.r.merge(&o.r);
    }
}

/// Users of `addr`, spread over load threads, that grow and shrink between
/// steps. Each sends a request every `think` (±50%, uniform), cycling
/// through `MIX` at random. Dropping it closes every connection.
pub struct Users {
    shared: Arc<Shared>,
    threads: Vec<thread::JoinHandle<()>>,
}

struct Shared {
    addr: SocketAddr,
    /// `MIX`'s requests, in its order.
    requests: Vec<Vec<u8>>,
    think_us: u64,
    threads: usize,
    base: Instant,
    target: AtomicUsize,
    live: AtomicUsize,
    /// The window measured, in ms since `base`: start << 32 | end.
    window: AtomicU64,
    /// Bumped to ask every thread for what it counted.
    collect: AtomicUsize,
    /// Threads that answered, and their sum.
    collected: Mutex<(usize, Step)>,
    stop: AtomicBool,
}

/// One thread's users.
struct Local {
    /// How many this thread should have: those numbered past it leave.
    target: Cell<usize>,
    live: Cell<usize>,
    step: RefCell<Step>,
}

impl Users {
    pub fn start(addr: SocketAddr, threads: usize, think: Duration) -> Users {
        let shared = Arc::new(Shared {
            addr,
            requests: MIX
                .iter()
                .map(|(path, _)| get_request(addr, path).into_bytes())
                .collect(),
            think_us: (think.as_micros() as u64).max(1000),
            threads: threads.max(1),
            base: Instant::now(),
            target: AtomicUsize::new(0),
            live: AtomicUsize::new(0),
            window: AtomicU64::new(0),
            collect: AtomicUsize::new(0),
            collected: Mutex::default(),
            stop: AtomicBool::new(false),
        });
        let threads = (0..shared.threads)
            .map(|t| {
                let shared = shared.clone();
                thread::spawn(move || LocalSet::new().block_on(&runtime(), manage(shared, t)))
            })
            .collect();
        Users { shared, threads }
    }

    /// Grows or shrinks to `n` users, then waits `settle`. New users connect
    /// spread over a second; leaving ones go at their next request's time.
    pub fn resize(&self, n: usize, settle: Duration) {
        let sh = &self.shared;
        sh.target.store(n, Relaxed);
        let t = Instant::now();
        let most = Duration::from_micros(2 * sh.think_us) + TIMEOUT;
        while sh.live.load(Relaxed) != n && t.elapsed() < most {
            thread::sleep(Duration::from_millis(10));
        }
        thread::sleep(settle);
    }

    /// Counts the requests due in the next `time`, and what came of them by
    /// a second after it.
    pub fn measure(&self, time: Duration) -> Step {
        let sh = &self.shared;
        *sh.collected.lock().unwrap() = (0, Step::default());
        let start = sh.base.elapsed().as_millis() as u64 + 1;
        let end = start + time.as_millis() as u64;
        sh.window.store(start << 32 | end, Relaxed);
        let over = sh.base + Duration::from_millis(end) + GRACE;
        thread::sleep(over.saturating_duration_since(Instant::now()));
        sh.collect.fetch_add(1, Relaxed);
        loop {
            {
                let mut c = sh.collected.lock().unwrap();
                if c.0 == sh.threads {
                    let mut step = std::mem::take(&mut c.1);
                    step.users = sh.live.load(Relaxed);
                    return step;
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Users {
    fn drop(&mut self) {
        self.shared.stop.store(true, Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

/// A thread's users: starts those it lacks every 50 ms, and hands in what
/// they counted when asked.
async fn manage(sh: Arc<Shared>, t: usize) {
    let local = Rc::new(Local {
        target: Cell::new(0),
        live: Cell::new(0),
        step: RefCell::default(),
    });
    // Each user's next due time, for what the ones stuck behind still owe.
    let mut due: Vec<Rc<Cell<u64>>> = Vec::new();
    let mut rng = Rng::new(t as u64 + 1);
    let mut epoch = 0;
    while !sh.stop.load(Relaxed) {
        let target = sh.target.load(Relaxed);
        local.target.set((target + sh.threads - 1 - t) / sh.threads);
        while local.live.get() < local.target.get() {
            let id = local.live.get();
            local.live.set(id + 1);
            sh.live.fetch_add(1, Relaxed);
            let next = Rc::new(Cell::new(u64::MAX));
            due.push(next.clone());
            let n = id * sh.threads + t;
            tokio::task::spawn_local(user(sh.clone(), local.clone(), id, n, next, rng.next()));
        }
        if sh.collect.load(Relaxed) != epoch {
            epoch = sh.collect.load(Relaxed);
            due.retain(|d| Rc::strong_count(d) > 1);
            let mut step = local.step.take();
            step.scheduled += owed(&due, sh.window.load(Relaxed), sh.think_us);
            let mut c = sh.collected.lock().unwrap();
            c.0 += 1;
            c.1.merge(&step);
        }
        sleep(Duration::from_millis(50)).await;
    }
}

/// The window's start and end in µs since `base`.
fn bounds(window: u64) -> (u64, u64) {
    ((window >> 32) * 1000, (window & 0xffff_ffff) * 1000)
}

/// Whether a request due at `due` counts in `window`, `now` being when it
/// is counted (all in µs since `base`).
fn counts(window: u64, due: u64, now: u64) -> bool {
    let (start, end) = bounds(window);
    (start..end).contains(&due) && now < end + GRACE.as_micros() as u64
}

/// Requests due in `window` that users stuck waiting on the server have not
/// reached: as many as fit from each one's next due time to the window's
/// end, at the mean interval.
fn owed(due: &[Rc<Cell<u64>>], window: u64, think_us: u64) -> u64 {
    let (start, end) = bounds(window);
    due.iter()
        .map(|d| end.saturating_sub(d.get().max(start)).div_ceil(think_us))
        .sum()
}

/// One user, numbered `id` on its thread and `n` in all, until its thread
/// wants fewer than `id + 1`.
async fn user(
    sh: Arc<Shared>,
    local: Rc<Local>,
    id: usize,
    n: usize,
    next: Rc<Cell<u64>>,
    seed: u64,
) {
    let mut rng = Rng::new(seed);
    let now = || sh.base.elapsed().as_micros() as u64;
    let at = |us: u64| (sh.base + Duration::from_micros(us)).into();
    // Connections open spread over a second, and each first request comes
    // at a random point of one interval after.
    sleep_until(at(now() + rng.below(1_000_000))).await;
    let mut deadline = Deadline::new();
    let mut conn = deadline
        .within(async { connect(sh.addr, n).await.ok() })
        .await;
    let mut buf = Vec::new();
    let mut done = now();
    let mut due = done + rng.below(sh.think_us);
    loop {
        next.set(due);
        let waited = done < due;
        sleep_until(at(due)).await;
        if id >= local.target.get() {
            break;
        }
        let sent = now();
        if counts(sh.window.load(Relaxed), due, sent) {
            let mut step = local.step.borrow_mut();
            step.scheduled += 1;
            if waited {
                step.late.record(sent - due);
            }
        }
        let this = due;
        due += sh.think_us / 2 + rng.below(sh.think_us);
        next.set(due);
        let request = &sh.requests[pick(rng.below(10))];
        let r = deadline
            .within(exchange(&mut conn, sh.addr, n, request, &mut buf))
            .await;
        if r.as_ref().is_none_or(|r| r.close) {
            conn = None;
        }
        done = now();
        if counts(sh.window.load(Relaxed), this, done) {
            let took = Duration::from_micros(done - this);
            record(&mut local.step.borrow_mut().r, r, took);
        }
    }
    local.live.set(local.live.get() - 1);
    sh.live.fetch_sub(1, Relaxed);
}

/// Sends `request` on `conn`, connecting first if it has none, and reads
/// the response.
async fn exchange(
    conn: &mut Option<TcpStream>,
    addr: SocketAddr,
    n: usize,
    request: &[u8],
    buf: &mut Vec<u8>,
) -> Option<Response> {
    if conn.is_none() {
        *conn = Some(connect(addr, n).await.ok()?);
    }
    let s = conn.as_mut()?;
    s.write_all(request).await.ok()?;
    read(s, buf).await
}

/// `connections` closed loops for `time`, each opening a connection per
/// request as a client without keep-alive does: it sends `request` (which
/// asks the server to close), reads the response and waits for the server
/// to close, a second at most (else an error). Counts what completes in
/// the window.
pub fn churn(
    addr: SocketAddr,
    request: &str,
    connections: usize,
    threads: usize,
    time: Duration,
) -> Report {
    let end = Instant::now() + time;
    loops(
        request.as_bytes(),
        connections,
        threads,
        time,
        |n, request| churn_loop(addr, n, request, end),
    )
}

async fn churn_loop(addr: SocketAddr, n: usize, request: Rc<[u8]>, end: Instant) -> Report {
    let mut report = Report::default();
    let mut deadline = Deadline::new();
    let mut buf = Vec::new();
    while Instant::now() < end {
        let t = Instant::now();
        let r = deadline.within(once(addr, n, &request, &mut buf)).await;
        if Instant::now() >= end {
            break;
        }
        record(&mut report, r, t.elapsed());
    }
    report
}

/// One request on a connection of its own, which the server then closes.
async fn once(addr: SocketAddr, n: usize, request: &[u8], buf: &mut Vec<u8>) -> Option<Response> {
    let mut s = connect(addr, n).await.ok()?;
    s.write_all(request).await.ok()?;
    let r = read(&s, buf).await?;
    let closed = timeout(Duration::from_secs(1), closed(&s)).await;
    closed.is_ok().then_some(r)
}

/// Returns once the peer closes or resets `s`, dropping what it sends.
async fn closed(s: &TcpStream) {
    loop {
        if s.readable().await.is_err() {
            return;
        }
        match BLOCK.with_borrow_mut(|block| s.try_read(block)) {
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Ok(1..) => {}
            _ => return,
        }
    }
}

/// `connections` keep-alive closed loops of `request` (head and body) for
/// `warmup + time`, as `run` does but on a few threads, so a thousand
/// connections cost no thousand threads. Counts the requests sent in
/// `time`, waiting for the last of them (`TIMEOUT` at most), and connects
/// refused apart: a server told to stop in `time` fails those it drops,
/// then refuses the rest once it stops listening.
pub fn closed_loop(
    addr: SocketAddr,
    request: &[u8],
    connections: usize,
    threads: usize,
    warmup: Duration,
    time: Duration,
) -> Report {
    let start = Instant::now() + warmup;
    let end = start + time;
    loops(request, connections, threads, time, |n, request| {
        keep_alive_loop(addr, n, request, start, end)
    })
}

async fn keep_alive_loop(
    addr: SocketAddr,
    n: usize,
    request: Rc<[u8]>,
    start: Instant,
    end: Instant,
) -> Report {
    let mut report = Report::default();
    let mut conn = None;
    let mut deadline = Deadline::new();
    let mut buf = Vec::new();
    loop {
        let sent = Instant::now();
        if sent >= end {
            return report;
        }
        if conn.is_none() {
            let c = deadline.within(async { Some(connect(addr, n).await) });
            match c.await {
                Some(Ok(s)) => conn = Some(s),
                c => {
                    let refused =
                        matches!(c, Some(Err(e)) if e.kind() == io::ErrorKind::ConnectionRefused);
                    if sent >= start {
                        report.refused += u64::from(refused);
                        report.errors += u64::from(!refused);
                    }
                    // Refused connects fail at once: no spinning on them.
                    sleep(Duration::from_millis(10)).await;
                    continue;
                }
            }
        }
        let r = deadline
            .within(exchange(&mut conn, addr, n, &request, &mut buf))
            .await;
        if r.as_ref().is_none_or(|r| r.close) {
            conn = None;
        }
        if sent >= start {
            record(&mut report, r, sent.elapsed());
        }
    }
}

/// `connections` tasks of `task(n, request)`, the `n`th on load thread
/// `n % threads`, and the sum of their reports over `time`.
fn loops<F: Future<Output = Report> + 'static>(
    request: &[u8],
    connections: usize,
    threads: usize,
    time: Duration,
    task: impl Fn(usize, Rc<[u8]>) -> F + Sync,
) -> Report {
    let threads = threads.max(1);
    let task = &task;
    let mut total = Report {
        seconds: time.as_secs_f64(),
        ..Report::default()
    };
    for r in spread(threads, |t| async move {
        let request: Rc<[u8]> = request.into();
        let mut set = JoinSet::new();
        for n in (t..connections).step_by(threads) {
            set.spawn_local(task(n, request.clone()));
        }
        sum(set).await
    }) {
        total.merge(&r);
    }
    total
}

/// What `r`, which took `took`, adds to `report`.
fn record(report: &mut Report, r: Option<Response>, took: Duration) {
    match r {
        Some(r) if (200..300).contains(&r.status) => {
            report.ok += 1;
            report.bytes += r.len as u64;
            report.latency.record(took.as_micros() as u64);
        }
        Some(_) => report.non_2xx += 1,
        None => report.errors += 1,
    }
}

/// The sum of what every task in `set` reported.
async fn sum(mut set: JoinSet<Report>) -> Report {
    let mut sum = Report::default();
    while let Some(r) = set.join_next().await {
        sum.merge(&r.expect("load task"));
    }
    sum
}

/// What `ws` measured.
pub struct Ws {
    pub opened: usize,
    /// The echoes: each 2xx here is a message that came back as sent.
    pub echo: Report,
}

/// The text the WebSocket test echoes: 32 bytes.
const MESSAGE: &[u8] = b"wisp-load websocket echo 32 byte";

/// Opens `sockets` WebSockets to `path`, 64 handshakes at a time per
/// thread, runs `idle` while all sit open (to read the server's memory),
/// then has `active` of them echo a 32-byte text message closed loop for
/// `time`.
pub fn ws(
    addr: SocketAddr,
    path: &'static str,
    sockets: usize,
    active: usize,
    threads: usize,
    time: Duration,
    idle: impl FnOnce(),
) -> Ws {
    let threads = threads.max(1);
    let barrier = std::sync::Barrier::new(threads + 1);
    thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|t| {
                let barrier = &barrier;
                s.spawn(move || {
                    LocalSet::new().block_on(&runtime(), async move {
                        let mut set = JoinSet::new();
                        let mut open = Vec::new();
                        for n in (t..sockets).step_by(threads) {
                            if set.len() >= 64
                                && let Some(Ok(Ok(s))) = set.join_next().await
                            {
                                open.push(s);
                            }
                            set.spawn_local(ws_open(addr, n, path));
                        }
                        while let Some(r) = set.join_next().await {
                            if let Ok(Ok(s)) = r {
                                open.push(s);
                            }
                        }
                        let opened = open.len();
                        // Blocking this thread is fine: its sockets are idle.
                        barrier.wait();
                        barrier.wait();
                        let end = Instant::now() + time;
                        let mine = (t..active).step_by(threads).count().min(opened);
                        let rest = open.split_off(mine);
                        let mut set = JoinSet::new();
                        for s in open {
                            set.spawn_local(ws_echo(s, end));
                        }
                        let echo = sum(set).await;
                        drop(rest);
                        (opened, echo)
                    })
                })
            })
            .collect();
        barrier.wait();
        idle();
        barrier.wait();
        let mut out = Ws {
            opened: 0,
            echo: Report {
                seconds: time.as_secs_f64(),
                ..Report::default()
            },
        };
        for h in handles {
            let (opened, echo) = h.join().expect("load thread panicked");
            out.opened += opened;
            out.echo.merge(&echo);
        }
        out
    })
}

/// Whether `path` is a WebSocket that echoes: `Err` with the handshake's
/// status (404 when there is none) or what went wrong.
pub fn ws_check(addr: SocketAddr, path: &'static str) -> Result<(), String> {
    let mut r = spread(1, |_| async move {
        let mut s = ws_open(addr, 0, path).await?;
        s.write_all(&text_frame(MESSAGE))
            .await
            .map_err(|e| e.to_string())?;
        match timeout(TIMEOUT, read_frame(&s)).await {
            Ok(Some(m)) if m == MESSAGE => Ok(()),
            Ok(Some(m)) => Err(format!("echoed {:?}", String::from_utf8_lossy(&m))),
            _ => Err("no echo".into()),
        }
    });
    r.pop().expect("one thread")
}

/// A WebSocket: the handshake answered 101, else its status or the error.
async fn ws_open(addr: SocketAddr, n: usize, path: &str) -> Result<TcpStream, String> {
    let handshake = format!(
        "GET {path} HTTP/1.1\r\nhost: {addr}\r\nupgrade: websocket\r\nconnection: Upgrade\r\nsec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\nsec-websocket-version: 13\r\n\r\n"
    );
    let go = async {
        let mut s = connect(addr, n).await.map_err(|e| e.to_string())?;
        s.write_all(handshake.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        match read(&s, &mut Vec::new()).await {
            Some(r) if r.status == 101 => Ok(s),
            Some(r) => Err(r.status.to_string()),
            None => Err("no answer to the handshake".into()),
        }
    };
    timeout(TIMEOUT, go)
        .await
        .unwrap_or_else(|_| Err("handshake timed out".into()))
}

/// `s` echoing `MESSAGE` closed loop until `end`.
async fn ws_echo(mut s: TcpStream, end: Instant) -> Report {
    let frame = text_frame(MESSAGE);
    let mut report = Report::default();
    while Instant::now() < end {
        let t = Instant::now();
        let echo = timeout(TIMEOUT, async {
            s.write_all(&frame).await.ok()?;
            read_frame(&s).await
        })
        .await
        .ok()
        .flatten();
        match echo {
            Some(m) if m == MESSAGE => {
                report.ok += 1;
                report.bytes += m.len() as u64;
                report.latency.record(t.elapsed().as_micros() as u64);
            }
            Some(_) => report.non_2xx += 1,
            None => {
                report.errors += 1;
                break;
            }
        }
    }
    report
}

/// A masked text frame of `payload` (under 126 bytes), as a client sends.
fn text_frame(payload: &[u8]) -> Vec<u8> {
    let mask = [0x37, 0xfa, 0x21, 0x3d];
    let mut f = vec![0x81, 0x80 | payload.len() as u8];
    f.extend(mask);
    f.extend(payload.iter().zip(mask.iter().cycle()).map(|(b, m)| b ^ m));
    f
}

/// The frame at the start of `buf`: its opcode, payload and length in all;
/// `None` until all of it is there. A server's frames are not masked.
fn frame(buf: &[u8]) -> Option<(u8, &[u8], usize)> {
    let (op, len) = (buf.first()? & 0x0f, buf.get(1)? & 0x7f);
    let (len, at) = match len {
        126 => (
            u16::from_be_bytes(buf.get(2..4)?.try_into().ok()?) as usize,
            4,
        ),
        127 => (
            u64::from_be_bytes(buf.get(2..10)?.try_into().ok()?) as usize,
            10,
        ),
        n => (n as usize, 2),
    };
    Some((op, buf.get(at..at + len)?, at + len))
}

/// The payload of the next text or binary frame (pings and pongs are
/// skipped); `None` on a close, an error or the end.
async fn read_frame(s: &TcpStream) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    loop {
        while let Some((op, payload, len)) = frame(&buf) {
            match op {
                1 | 2 => return Some(payload.to_vec()),
                8 => return None,
                _ => {
                    buf.drain(..len);
                }
            }
        }
        s.readable().await.ok()?;
        BLOCK.with_borrow_mut(|block| match s.try_read(block) {
            Ok(0) => None,
            Ok(n) => {
                buf.extend_from_slice(&block[..n]);
                Some(())
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Some(()),
            Err(_) => None,
        })?;
    }
}

/// How a server took a request sent on a connection of its own.
#[derive(Debug, PartialEq)]
pub enum Answer {
    Status(u16),
    /// Closed or reset before any answer.
    Closed,
    /// No answer, nor a close, in the time given; or no connection.
    Silent,
}

/// Sends `bytes` (anything at all) and waits `wait` for the start of an
/// answer. A server may answer and close before taking all of it, so what
/// came is read either way.
pub fn raw(addr: SocketAddr, bytes: &[u8], wait: Duration) -> Answer {
    use std::io::{Read, Write};
    let Ok(mut s) = std::net::TcpStream::connect_timeout(&addr, wait) else {
        return Answer::Silent;
    };
    let _ = s.set_read_timeout(Some(wait));
    let _ = s.set_write_timeout(Some(wait));
    let _ = s.write_all(bytes);
    let mut buf = Vec::new();
    let mut block = [0; 4096];
    loop {
        if buf.len() >= 12 && buf.starts_with(b"HTTP/1.") {
            return std::str::from_utf8(&buf[9..12])
                .ok()
                .and_then(|s| s.parse().ok())
                .map_or(Answer::Closed, Answer::Status);
        }
        match s.read(&mut block) {
            Ok(0) => return Answer::Closed,
            Ok(n) => buf.extend_from_slice(&block[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Answer::Silent;
            }
            Err(_) => return Answer::Closed,
        }
    }
}

/// `clients` connections, opened over a second, each sending a `GET path`
/// head a byte every 500 ms and never ending it (header lines follow the
/// host, one after another), for `time`. Returns how many the server
/// refused, answered or closed before then.
pub fn slow(addr: SocketAddr, path: &str, clients: usize, threads: usize, time: Duration) -> usize {
    let head = format!("GET {path} HTTP/1.1\r\nhost: {addr}\r\n");
    let start = Instant::now();
    let threads = threads.max(1);
    let cut = spread(threads, |t| {
        let head = head.as_bytes();
        async move {
            let head: Rc<[u8]> = head.into();
            let mut rng = Rng::new(t as u64 + 1);
            let mut set = JoinSet::new();
            for n in (t..clients).step_by(threads) {
                let at = start + Duration::from_micros(rng.below(1_000_000));
                set.spawn_local(slowloris(addr, n, head.clone(), at, start + time));
            }
            let mut cut = 0;
            while let Some(r) = set.join_next().await {
                cut += usize::from(r.unwrap_or(true));
            }
            cut
        }
    });
    cut.iter().sum()
}

/// One slow client: true if the server cut it off before `end`.
async fn slowloris(addr: SocketAddr, n: usize, head: Rc<[u8]>, at: Instant, end: Instant) -> bool {
    sleep_until(at.into()).await;
    let Ok(Ok(mut s)) = timeout(TIMEOUT, connect(addr, n)).await else {
        return true;
    };
    let mut tick = Instant::now();
    for &b in head.iter().chain(b"x-a: b\r\n".iter().cycle()) {
        if s.write_all(&[b]).await.is_err() {
            return true;
        }
        tick += Duration::from_millis(500);
        if tick >= end {
            return false;
        }
        // Till the next byte, watch for the server answering or closing.
        loop {
            match timeout_at(tick.into(), s.readable()).await {
                Err(_) => break,
                Ok(Err(_)) => return true,
                Ok(Ok(())) => match BLOCK.with_borrow_mut(|block| s.try_read(block)) {
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    // An answer (a 408 or a 400), a close or a reset.
                    _ => return true,
                },
            }
        }
    }
    false
}

/// A connection to `addr`, the `n`th of a run. On Linux it comes from one
/// of 127.0.0.2-255 in turn: each source address has its own ephemeral
/// ports, so 50k connections and a churn do not run out of them. It closes
/// with a reset.
async fn connect(addr: SocketAddr, n: usize) -> io::Result<TcpStream> {
    let s = if addr.is_ipv4() {
        TcpSocket::new_v4()?
    } else {
        TcpSocket::new_v6()?
    };
    s.set_nodelay(true)?;
    s.set_zero_linger()?;
    if cfg!(target_os = "linux") && addr.is_ipv4() && addr.ip().is_loopback() {
        let from = Ipv4Addr::new(127, 0, 0, 2 + (n % 254) as u8);
        s.bind(SocketAddr::new(from.into(), 0))?;
    }
    s.connect(addr).await
}

/// Reads one response into `buf` (the caller's, kept between its requests:
/// it grows to one response and no more). Bytes past the response are
/// dropped: nothing is pipelined here.
async fn read(s: &TcpStream, buf: &mut Vec<u8>) -> Option<Response> {
    buf.clear();
    loop {
        if let Some(r) = parse(buf)? {
            return Some(r);
        }
        s.readable().await.ok()?;
        buf.reserve(4096);
        match s.try_read_buf(buf) {
            Ok(0) => return None,
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(_) => return None,
        }
    }
}

/// Runs `task(t)` on each of `threads` threads, each a runtime of its own,
/// and returns what each returned.
fn spread<T: Send, F: Future<Output = T>>(
    threads: usize,
    task: impl Fn(usize) -> F + Sync,
) -> Vec<T> {
    thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|t| {
                let task = &task;
                s.spawn(move || LocalSet::new().block_on(&runtime(), task(t)))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("load thread panicked"))
            .collect()
    })
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a tokio runtime")
}

/// xorshift64*: cheap, and the same draws every run.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `0..n`, for `n` under 2^32.
    fn below(&mut self, n: u64) -> u64 {
        ((self.next() >> 32) * n) >> 32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_proportions() {
        let mut seen = [0u64; 3];
        let mut rng = Rng::new(7);
        for _ in 0..100_000 {
            seen[pick(rng.below(10))] += 1;
        }
        for (i, &(_, weight)) in MIX.iter().enumerate() {
            let share = seen[i] as f64 / 100_000.0;
            assert!((share - weight as f64 / 10.0).abs() < 0.01, "{i}: {share}");
        }
        assert_eq!(
            (0..10).map(pick).collect::<Vec<_>>(),
            [0, 0, 0, 0, 0, 1, 1, 1, 2, 2]
        );
    }

    #[test]
    fn pass_rule() {
        let step = |scheduled, ok, non_2xx, errors, us: &[u64]| {
            let mut s = Step {
                scheduled,
                r: Report {
                    ok,
                    non_2xx,
                    errors,
                    ..Report::default()
                },
                ..Step::default()
            };
            us.iter().for_each(|&v| s.r.latency.record(v));
            s
        };
        let fast = [1_000; 100];
        assert!(step(10_000, 10_000, 0, 0, &fast).passes(100_000));
        // p99 over the SLO: 2 slow of 100.
        let mut slow = fast;
        slow[..2].fill(200_000);
        assert!(!step(10_000, 10_000, 0, 0, &slow).passes(100_000));
        // 0.1% failed passes, more does not.
        assert!(step(10_000, 9_990, 5, 5, &fast).passes(100_000));
        assert!(!step(10_000, 9_989, 6, 5, &fast).passes(100_000));
        // 99% answered passes, less does not.
        assert!(step(10_000, 9_900, 0, 0, &fast).passes(100_000));
        assert!(!step(10_000, 9_899, 0, 0, &fast).passes(100_000));
        assert!(!step(0, 0, 0, 0, &[]).passes(100_000));
    }

    #[test]
    fn websocket_frames() {
        // A server's text frame, a ping, and a 300-byte binary frame.
        assert_eq!(frame(b"\x81\x02hi"), Some((1, &b"hi"[..], 4)));
        assert_eq!(frame(b"\x81\x02h"), None);
        assert_eq!(frame(b"\x89\x00"), Some((9, &b""[..], 2)));
        let mut long = vec![0x82, 126, 1, 44];
        long.extend([7; 300]);
        assert_eq!(
            frame(&long).map(|f| (f.0, f.1.len(), f.2)),
            Some((2, 300, 304))
        );
        assert_eq!(frame(&long[..200]), None);
        // A client's frame is masked: unmasking it gives the text back.
        let f = text_frame(MESSAGE);
        assert_eq!((f[0], f[1] as usize), (0x81, 0x80 | MESSAGE.len()));
        let text: Vec<u8> = f[6..]
            .iter()
            .zip(f[2..6].iter().cycle())
            .map(|(b, m)| b ^ m)
            .collect();
        assert_eq!(text, MESSAGE);
    }

    #[test]
    fn windows_count_due_times() {
        let w = 2 << 32 | 12; // 2 ms to 12 ms
        assert!(counts(w, 2_000, 2_000));
        assert!(!counts(w, 1_999, 5_000));
        assert!(!counts(w, 12_000, 12_000));
        assert!(counts(w, 11_999, 1_011_999), "finishing in the grace");
        assert!(!counts(w, 11_999, 1_012_000));
        let due = |us| Rc::new(Cell::new(us));
        // On time, stuck since before the window, stuck in it, not started.
        let users = [due(20_000), due(0), due(7_000), due(u64::MAX)];
        assert_eq!(owed(&users, w, 1_000), 10 + 5);
    }
}
