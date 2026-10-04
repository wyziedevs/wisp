//! The edge build: the app compiled to `wasm32-unknown-unknown` and served
//! by a JavaScript host (Cloudflare Workers, Deno Deploy, Vercel and Netlify
//! edge functions, Node). `wisp build --target <host>` writes the shim.
//!
//! No wasm-bindgen: the ABI is a few numbers, driven by `bridge.js`.
//!
//! Exports:
//! - `main()`: std's; runs the app's `main`, whose `wisp::run` lands in
//!   [`start`] and begins `init`. Call once, after `wisp_env`.
//! - `wisp_buf(len) -> ptr`: room for `len` bytes, which the host writes
//!   (the same address, while it asks for no more than it has)
//!   before the call that reads them.
//! - `wisp_env(len)`: the environment, `KEY=value` entries ended by NUL.
//! - `wisp_request(id, len)`: a request, `METHOD target peer` then headers,
//!   `name: value` a line each, an empty line and the body.
//! - `wisp_fetched(id, len)`: the answer to import `fetch`, `status`, then as
//!   above (status 0: failed, the body says why).
//! - `wisp_timer(id)`: import `timer`'s time has come.
//! - `wisp_cancel(id)`: the client of request `id` left; its task is dropped,
//!   so a streamed body's sender fails.
//! - `wisp_pull(id)`: request `id`'s stream wants chunks again.
//!
//! - `wisp_current() -> id`: after a trap, the task that trapped: a
//!   request's id, or `u32::MAX` for `init`.
//! - `wisp_poll()`: after a trap, polls the tasks woken meanwhile.
//!
//! Imports (module `wisp`): `random(ptr, len)`, `now() -> f64` (seconds since 1970), `log(ptr, len)`, `reply(id, head, head_len, head_id, body, body_len)` (the reply to
//! request `id`: its head, `status` and `name: value` lines, and its number
//! among the heads sent, `u32::MAX` if not kept; a first line `200 stream` means the
//! body follows as `chunk(id, ptr, len)` calls, the last one empty; a chunk
//! returns 0 when the client is behind, and none follows until `wisp_pull`),
//! `fetch(id, ptr, len)` (a request, its target a URL), `timer(id, ms)`.
//!
//! Tasks: a request's has its id; `init`'s is `u32::MAX`; those of
//! `wisp::spawn` count up from `1 << 31`.
//!
//! Tasks are polled when the host calls in, and never between: every
//! wakeup comes from a request or a fetch arriving. A panic traps, which
//! fails the host's call; the shim answers 500 and starts a new instance.

// The one place Wisp is `unsafe`, in name only: `no_mangle` to export,
// `extern` to import. Every import is declared `safe` and takes plain
// numbers; memory crosses only as buffers Rust owns and hands out by address.
#![allow(unsafe_code)]

use crate::{App, Reply, Request};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll, Wake, Waker};

#[link(wasm_import_module = "wisp")]
unsafe extern "C" {
    safe fn random(ptr: *mut u8, len: usize);
    #[link_name = "log"]
    safe fn log_line(ptr: *const u8, len: usize);
    #[link_name = "reply"]
    safe fn send_reply(
        id: u32,
        head: *const u8,
        head_len: usize,
        head_id: u32,
        body: *const u8,
        body_len: usize,
    );
    #[link_name = "fetch"]
    safe fn send_fetch(id: u32, ptr: *const u8, len: usize);
    #[link_name = "chunk"]
    safe fn send_chunk(id: u32, ptr: *const u8, len: usize) -> u32;
    safe fn timer(id: u32, ms: u32);
    /// Seconds since 1970, the host's clock (`std`'s has none here).
    safe fn now() -> f64;
}

/// Whole seconds since 1970.
pub(crate) fn unix_seconds() -> u64 {
    now() as u64
}

/// The host's clock, for what `std::time::Instant` cannot time here.
pub(crate) fn clock() -> std::time::Duration {
    std::time::Duration::from_secs_f64(now())
}

type Task = Pin<Box<dyn Future<Output = ()>>>;
/// Starts the task of request `id`, given the bytes the host wrote.
type Handler = fn(u32, Vec<u8>);

/// The task that runs `init`; request ids come from the host and never
/// reach it.
const INIT: u32 = u32::MAX;

static HANDLER: OnceLock<Handler> = OnceLock::new();

thread_local! {
    static IN: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static ENV: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
    static TASKS: RefCell<HashMap<u32, Task>> = RefCell::new(HashMap::new());
    static WOKEN: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    static FETCHES: RefCell<HashMap<u32, Fetching>> = RefCell::new(HashMap::new());
    static NEXT_FETCH: Cell<u32> = const { Cell::new(0) };
    /// Timers not yet due, with what to wake; a due one is removed.
    static TIMERS: RefCell<HashMap<u32, Option<Waker>>> = RefCell::new(HashMap::new());
    static NEXT_TIMER: Cell<u32> = const { Cell::new(0) };
    /// Streams waiting for `wisp_pull`, by request.
    static PULLS: RefCell<HashMap<u32, Option<Waker>>> = RefCell::new(HashMap::new());
    static NEXT_TASK: Cell<u32> = const { Cell::new(0) };
    static CURRENT: Cell<u32> = const { Cell::new(INIT) };
    /// `init` has finished: 0 not yet, 1 well, 2 failed.
    static READY: Cell<u8> = const { Cell::new(0) };
    static WAITING: RefCell<Vec<Waker>> = const { RefCell::new(Vec::new()) };
    /// The waker polls hand out: one `Arc` for every task that does not keep it.
    static WAKE: RefCell<Arc<Wakeup>> = RefCell::new(Arc::new(Wakeup(AtomicU32::new(0))));
    /// The reply's head, written here, and the heads sent so far: a route
    /// answers with the same head each time, so the host is sent its number.
    static HEAD: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static HEADS: RefCell<Vec<Vec<u8>>> = const { RefCell::new(Vec::new()) };
    /// The last peer text and what it parsed to.
    static PEER: RefCell<(String, SocketAddr)> = const { RefCell::new((String::new(), LOCAL)) };
}

const LOCAL: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 0);

#[unsafe(no_mangle)]
pub extern "C" fn wisp_buf(len: usize) -> *mut u8 {
    IN.with_borrow_mut(|b| {
        if b.len() < len {
            b.resize(len, 0);
        }
        b.as_mut_ptr()
    })
}

/// The first `len` bytes the host wrote. The buffer stays where it is, so a
/// host that was given its address may write the next request there.
fn take_in(len: usize) -> Vec<u8> {
    IN.with_borrow(|b| b[..len].to_vec())
}

#[unsafe(no_mangle)]
pub extern "C" fn wisp_env(len: usize) {
    let text = String::from_utf8_lossy(&take_in(len)).into_owned();
    ENV.with_borrow_mut(|env| {
        env.extend(
            text.split('\0')
                .filter_map(|l| l.split_once('='))
                .map(|(k, v)| (k.to_string(), v.to_string())),
        );
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn wisp_request(id: u32, len: usize) {
    let bytes = take_in(len);
    match HANDLER.get() {
        Some(start) => start(id, bytes),
        None => spawn(id, Box::pin(finish(id, Reply::plain(500)))),
    }
}

/// The task of request `id`: borrows its parts from `bytes`, and answers.
fn request<A: App>(id: u32, bytes: Vec<u8>) {
    let task = async move {
        let reply = match view(&bytes) {
            Some((method, target, peer, headers, body)) => {
                Ready.await;
                match READY.get() {
                    1 => {
                        let headers = headers.map(|(n, v)| (n, v.as_bytes()));
                        crate::http::handle_parts::<A>(method, target, headers, body, peer).await
                    }
                    _ => Reply::plain(500),
                }
            }
            None => Reply::plain(400),
        };
        finish(id, reply).await;
    };
    spawn(id, Box::pin(task));
}

/// Sends `reply`, once its changes to saved tables are in `WISP_STORE`.
async fn finish(id: u32, mut reply: Reply) {
    if !crate::edge_store::Saved.await {
        reply = Reply::plain(500);
    }
    send(id, &reply);
    // A stream's chunks go out as they come; an empty one ends it.
    if let crate::Body::Stream(mut rx) = reply.body {
        while let Some(chunk) = rx.recv().await {
            if !chunk.is_empty() && send_chunk(id, chunk.as_ptr(), chunk.len()) == 0 {
                PULLS.with_borrow_mut(|p| p.insert(id, None));
                Wait(&PULLS, id).await;
            }
        }
        send_chunk(id, std::ptr::null(), 0);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn wisp_fetched(id: u32, len: usize) {
    let bytes = take_in(len);
    // An answer nobody waits for (its request was dropped) is not kept.
    let waker = FETCHES.with_borrow_mut(|f| match f.get_mut(&id) {
        Some(slot @ Fetching::Waiting(_)) => match std::mem::replace(slot, Fetching::Done(bytes)) {
            Fetching::Waiting(w) => w,
            Fetching::Done(_) => None,
        },
        _ => None,
    });
    if let Some(w) = waker {
        w.wake();
    }
    run();
}

#[unsafe(no_mangle)]
pub extern "C" fn wisp_timer(id: u32) {
    done(&TIMERS, id);
}

#[unsafe(no_mangle)]
pub extern "C" fn wisp_pull(id: u32) {
    done(&PULLS, id);
}

#[unsafe(no_mangle)]
pub extern "C" fn wisp_cancel(id: u32) {
    let task = TASKS.with_borrow_mut(|t| t.remove(&id));
    drop(task); // outside the borrow: dropping it may wake others
    run();
}

#[unsafe(no_mangle)]
pub extern "C" fn wisp_current() -> u32 {
    CURRENT.get()
}

#[unsafe(no_mangle)]
pub extern "C" fn wisp_poll() {
    run();
}

/// `wisp::run` in the edge build: remembers the app and starts `init`.
pub(crate) fn start<A: App>() {
    log_panics();
    if HANDLER.set(request::<A>).is_err() {
        return;
    }
    let init = async {
        // Saved tables' rows, before `init`, which may set a store of its own.
        let opened = match env("WISP_STORE") {
            Some(_) => crate::edge_store::open()
                .await
                .map_err(|e| std::io::Error::other(format!("WISP_STORE: {}", e.detail()))),
            None => Ok(()),
        };
        let ready = match opened {
            Ok(()) => crate::prepare::<A>().await,
            failed => failed,
        };
        let ready = match ready {
            Ok(()) => 1,
            Err(e) => {
                crate::http::log(format_args!("wisp: {e}"));
                2
            }
        };
        // `prepare` put the native panic hook in, which keeps quiet about
        // handler panics that `catch` reports; here a panic traps before
        // `catch` sees it, so every one is logged.
        log_panics();
        READY.set(ready);
        WAITING.take().into_iter().for_each(Waker::wake);
    };
    spawn(INIT, Box::pin(init));
}

fn log_panics() {
    std::panic::set_hook(Box::new(|info| {
        crate::http::log(format_args!("wisp: {info}"))
    }));
}

pub(crate) fn env(key: &str) -> Option<String> {
    ENV.with_borrow(|env| env.get(key).cloned())
}

pub(crate) fn log(line: &str) {
    log_line(line.as_ptr(), line.len());
}

pub(crate) fn fill_random(out: &mut [u8]) {
    random(out.as_mut_ptr(), out.len());
}

/// Makes an HTTP request from an edge host, with its `fetch`: to a database
/// over HTTP (D1, Turso, Supabase), an API. `req.target` is the whole URL.
///
/// ```ignore
/// let mut req = wisp::Request::new("POST", "https://api.example.com/rows");
/// req.header("authorization", &format!("Bearer {}", wisp::env("API_KEY").unwrap_or_default()));
/// req.body = json.into_bytes();
/// let reply = wisp::edge::fetch(req).await?;
/// ```
///
/// A request that got no answer (a bad URL, the network) is a 502.
pub async fn fetch(req: Request) -> crate::Result<Reply> {
    let id = NEXT_FETCH.get();
    NEXT_FETCH.set(id.wrapping_add(1));
    FETCHES.with_borrow_mut(|f| f.insert(id, Fetching::Waiting(None)));
    let mut wire = format!("{} {}\n", req.method, req.target).into_bytes();
    push_headers(
        &mut wire,
        req.headers.iter().map(|(n, v)| (n.as_str(), v.as_str())),
        &req.body,
    );
    send_fetch(id, wire.as_ptr(), wire.len());
    let bytes = std::future::poll_fn(|cx| {
        FETCHES.with_borrow_mut(|f| match f.remove(&id) {
            Some(Fetching::Done(b)) => Poll::Ready(b),
            _ => {
                f.insert(id, Fetching::Waiting(Some(cx.waker().clone())));
                Poll::Pending
            }
        })
    })
    .await;
    let (status, headers, body) = split(&bytes)
        .ok_or_else(|| crate::Error::new(502, "the host sent a fetch reply Wisp cannot read"))?;
    let status: u16 = status.parse().unwrap_or(0);
    if !(100..=999).contains(&status) {
        return Err(crate::Error::new(
            502,
            format!(
                "fetch {} failed: {}",
                req.target,
                String::from_utf8_lossy(body)
            ),
        ));
    }
    let headers = headers
        .map(|(n, v)| (Cow::Owned(n.to_string()), Cow::Owned(v.to_string())))
        .collect();
    Ok(Reply {
        status,
        headers,
        body: crate::Body::Bytes(body.to_vec()),
    })
}

enum Fetching {
    Waiting(Option<Waker>),
    Done(Vec<u8>),
}

/// Waits for `init`.
struct Ready;

impl Future for Ready {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<()> {
        if READY.get() != 0 {
            return Poll::Ready(());
        }
        WAITING.with_borrow_mut(|w| w.push(cx.waker().clone()));
        Poll::Pending
    }
}

/// Wakes the task whose id it holds.
struct Wakeup(AtomicU32);

impl Wake for Wakeup {
    fn wake(self: Arc<Self>) {
        WOKEN.with_borrow_mut(|w| w.push(self.0.load(Relaxed)));
    }
}

/// Polls a new task at once: most are done in that poll, and never stored.
fn spawn(id: u32, task: Task) {
    poll(id, task);
    run();
}

/// One poll of task `id`, which is kept if it is pending.
fn poll(id: u32, mut task: Task) {
    let shared = WAKE.with_borrow(Arc::clone);
    shared.0.store(id, Relaxed);
    let waker = Waker::from(Arc::clone(&shared));
    CURRENT.set(id);
    let pending = task
        .as_mut()
        .poll(&mut Context::from_waker(&waker))
        .is_pending();
    drop(waker);
    CURRENT.set(INIT); // a trap outside any task fails them all
    // A clone the task kept must go on waking it: the next poll gets a new one.
    if Arc::strong_count(&shared) > 2 {
        WAKE.with_borrow_mut(|w| *w = Arc::new(Wakeup(AtomicU32::new(0))));
    }
    if pending {
        TASKS.with_borrow_mut(|t| t.insert(id, task));
    }
}

/// `wisp::spawn`: polled first by the `run` under way, since it is
/// always called from a task.
pub(crate) fn spawn_task(task: Task) {
    let n = NEXT_TASK.get();
    NEXT_TASK.set((n + 1) % (INIT >> 1)); // stays below `INIT`
    let id = (1 << 31) + n;
    TASKS.with_borrow_mut(|t| t.insert(id, task));
    WOKEN.with_borrow_mut(|w| w.push(id));
}

type Waiting = std::thread::LocalKey<RefCell<HashMap<u32, Option<Waker>>>>;

/// Waits until its id is taken out of the map, by [`done`]; dropped, it
/// takes it out itself.
struct Wait(&'static Waiting, u32);

impl Future for Wait {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<()> {
        self.0.with_borrow_mut(|m| match m.get_mut(&self.1) {
            Some(w) => {
                *w = Some(cx.waker().clone());
                Poll::Pending
            }
            None => Poll::Ready(()),
        })
    }
}

impl Drop for Wait {
    fn drop(&mut self) {
        self.0.with_borrow_mut(|m| m.remove(&self.1));
    }
}

/// Ends the [`Wait`] for `id`, if there is one.
fn done(map: &'static Waiting, id: u32) {
    if let Some(Some(w)) = map.with_borrow_mut(|m| m.remove(&id)) {
        w.wake();
    }
    run();
}

/// `wisp::sleep`: the host's `setTimeout`.
pub(crate) async fn sleep(duration: std::time::Duration) {
    let id = NEXT_TIMER.get();
    NEXT_TIMER.set(id.wrapping_add(1));
    TIMERS.with_borrow_mut(|t| t.insert(id, None));
    timer(id, duration.as_millis().min(u32::MAX as u128) as u32);
    Wait(&TIMERS, id).await
}

/// Polls every woken task until none is, then sends the changes to saved
/// tables they made, if any. A task is taken out while it is polled, so it
/// may spawn, wake or fetch freely.
fn run() {
    loop {
        poll_woken();
        if !crate::edge_store::due() {
            return;
        }
        spawn_task(Box::pin(crate::edge_store::send()));
    }
}

fn poll_woken() {
    while let Some(id) = WOKEN.with_borrow_mut(Vec::pop) {
        if let Some(task) = TASKS.with_borrow_mut(|t| t.remove(&id)) {
            poll(id, task);
        }
    }
}

/// A request's parts: method, target, peer, headers and body.
#[allow(clippy::type_complexity)]
fn view(
    bytes: &[u8],
) -> Option<(
    &str,
    &str,
    SocketAddr,
    impl Iterator<Item = (&str, &str)>,
    &[u8],
)> {
    let (first, headers, body) = split(bytes)?;
    let (method, rest) = cut(first, b' ')?;
    let (target, at) = cut(rest, b' ').unwrap_or((rest, ""));
    Some((method, target, peer(at), headers, body))
}

/// `s` around its first `byte`, an ASCII one: a byte search, quicker than a
/// `char` pattern's.
fn cut(s: &str, byte: u8) -> Option<(&str, &str)> {
    let i = s.bytes().position(|b| b == byte)?;
    Some((&s[..i], &s[i + 1..]))
}

fn peer(text: &str) -> SocketAddr {
    PEER.with_borrow_mut(|(last, addr)| {
        if last != text {
            *addr = text.parse().ok().map_or(LOCAL, |ip| SocketAddr::new(ip, 0));
            last.clear();
            last.push_str(text);
        }
        *addr
    })
}

/// `first line`, `name: value` lines, an empty line, the body.
fn split(bytes: &[u8]) -> Option<(&str, impl Iterator<Item = (&str, &str)>, &[u8])> {
    let end = bytes.windows(2).position(|w| w == b"\n\n")?;
    let head = std::str::from_utf8(&bytes[..end]).ok()?;
    let (first, rest) = head.split_once('\n').unwrap_or((head, ""));
    let headers = rest
        .lines()
        .filter_map(|l| cut(l, b':'))
        .map(|(n, v)| (n.trim(), v.trim()));
    Some((first, headers, &bytes[end + 2..]))
}

fn push_headers<'a>(
    w: &mut Vec<u8>,
    headers: impl Iterator<Item = (&'a str, &'a str)>,
    body: &[u8],
) {
    for (n, v) in headers {
        for part in [n, ": ", v, "\n"] {
            w.extend_from_slice(part.as_bytes());
        }
    }
    w.push(b'\n');
    w.extend_from_slice(body);
}

/// Sends the reply: its head, `status` (then ` stream` for a stream, whose
/// body follows as chunks) and `name: value` lines, and its number among the
/// heads sent; then its body, where it is, uncopied.
fn send(id: u32, reply: &Reply) {
    let (ptr, len, head_id) = HEAD.with_borrow_mut(|w| {
        w.clear();
        crate::http::push_decimal(w, reply.status.into());
        if matches!(reply.body, crate::Body::Stream(_)) {
            w.extend_from_slice(b" stream");
        }
        w.push(b'\n');
        for (n, v) in &reply.headers {
            for part in [n.as_bytes(), b": ", v.as_bytes(), b"\n"] {
                w.extend_from_slice(part);
            }
        }
        (w.as_ptr(), w.len(), head_number(w))
    });
    let body = reply.bytes();
    send_reply(id, ptr, len, head_id, body.as_ptr(), body.len());
}

/// Where `head` is among those sent, added if new; `u32::MAX` when there are
/// too many (a head that changes each time) to keep.
fn head_number(head: &[u8]) -> u32 {
    HEADS.with_borrow_mut(|heads| match heads.iter().position(|h| h == head) {
        Some(i) => i as u32,
        None if heads.len() < 64 => {
            heads.push(head.to_vec());
            heads.len() as u32 - 1
        }
        None => u32::MAX,
    })
}
