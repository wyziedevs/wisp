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
//! - `wisp_request_lazy(id, len)`: the same with only `host` among the
//!   headers and no body; the app asks for the others as it reads them,
//!   through `header(id, name, name_len, out, cap)` (the value's length, at
//!   most `cap` of it written to `out`; `u32::MAX`: none) and
//!   `headers(id, out, cap)` (every one but `content-length` and
//!   `transfer-encoding`, as `name: value` lines; its length, as `header`).
//! - `wisp_fetched(id, len)`: the answer to import `fetch`, `status`, then as
//!   above (status 0: failed, the body says why).
//! - `wisp_timer(id)`: import `timer`'s time has come.
//! - `wisp_cancel(id)`: the client of request `id` left; its task is dropped,
//!   so a streamed body's sender fails.
//! - `wisp_pull(id)`: request `id`'s stream wants chunks again.
//!
//! - `wisp_conn_open(id, len)`, `wisp_conn_data(id, len)`, `wisp_conn_close(id)`,
//!   `wisp_conn_pull(id)`: a connection the host accepted, whose raw bytes
//!   Wisp parses and answers itself (see [`crate::http::Raw`]): `open` with
//!   the peer's address as text, `data` with the bytes read, `close` when the
//!   client left, `pull` when it takes writes again after `conn_write`
//!   returned 0. Ids are below `1 << 30`.
//! - `wisp_current() -> id`: after a trap, the task that trapped: a
//!   request's id, or `u32::MAX` for `init`.
//! - `wisp_poll()`: after a trap, polls the tasks woken meanwhile.
//!
//! A request for `/_wisp/cron/<schedule>` is a host's cron trigger: it runs
//! the app's `wisp::cron` tasks of that schedule and the queues' due jobs
//! (see `jobs`), and answers 204 only with `Authorization: Bearer $CRON_SECRET`.
//!
//! Imports (module `wisp`): `random(ptr, len)`, `now() -> f64` (seconds since 1970), `log(ptr, len)`, `reply(id, head, head_len, head_id, body, body_len)` (the reply to
//! request `id`: its head, `status` and `name: value` lines, and its number
//! among the heads sent, `u32::MAX` if not kept; a first line `200 stream` means the
//! body follows as `chunk(id, ptr, len)` calls, the last one empty; a chunk
//! returns 0 when the client is behind, and none follows until `wisp_pull`;
//! `200 const` says the answer is the same to every request for that path
//! (without a query) that sends no `x-wisp-error` and answers `if-none-match`
//! by its ETag: a baked page, a trailing-slash redirect; see [`constant`]),
//! `fetch(id, ptr, len)` (a request, its target a URL), `timer(id, ms)`,
//! `conn_write(id, ptr, len) -> ok` (bytes for connection `id`, copied before
//! it returns; an empty write ends the connection; 0 when the client is
//! behind, and none follows until `wisp_conn_pull`).
//!
//! Tasks: a request's has its id; a connection's is `1 << 30` and its id;
//! `init`'s is `u32::MAX`; those of `wisp::spawn` count up from `1 << 31`.
//!
//! Tasks are polled when the host calls in, and never between: every
//! wakeup comes from a request or a fetch arriving. A panic traps, which
//! fails the host's call; the shim answers 500 and starts a new instance.

// The one place Wisp is `unsafe`, in name only: `no_mangle` to export,
// `extern` to import. Every import is declared `safe` and takes plain
// numbers; memory crosses only as buffers Rust owns and hands out by address.
#![allow(unsafe_code)]

use crate::http::edge_conn::{Raw, Step, Stream};
use crate::{App, Reply, Request};
use std::borrow::Cow;
use std::cell::{Cell, OnceCell, RefCell};
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
    safe fn conn_write(id: u32, ptr: *const u8, len: usize) -> u32;
    /// Seconds since 1970, the host's clock (`std`'s has none here).
    safe fn now() -> f64;
    #[link_name = "header"]
    safe fn get_header(id: u32, name: *const u8, name_len: usize, out: *mut u8, cap: usize) -> u32;
    #[link_name = "headers"]
    safe fn get_headers(id: u32, out: *mut u8, cap: usize) -> u32;
}

/// What `fill(out, cap)` writes, given room for it: `None` for `u32::MAX`.
fn fetched(fill: impl Fn(*mut u8, usize) -> u32) -> Option<String> {
    let mut b = vec![0; 256];
    loop {
        let n = fill(b.as_mut_ptr(), b.len());
        if n == u32::MAX {
            return None;
        }
        if n as usize <= b.len() {
            b.truncate(n as usize);
            return String::from_utf8(b).ok();
        }
        b = vec![0; n as usize];
    }
}

/// A header asked for by name, and its value if it has one.
type Named = (Box<str>, Option<Box<str>>);

/// Names [`Lazy`] asks for by name, kept with their values.
const NAMED: usize = 8;

/// The headers of a `wisp_request_lazy` request, fetched from the host as
/// they are read, each once: what `Cx` reads where the wire has none.
pub(crate) struct Lazy {
    id: u32,
    known: [OnceCell<Option<Box<str>>>; crate::cx::KNOWN],
    named: [OnceCell<Named>; NAMED],
    all: OnceCell<Vec<(Box<str>, Box<str>)>>,
    /// The route's guard ran (see [`guarded`]).
    guarded: Cell<bool>,
}

impl Lazy {
    fn new(id: u32) -> Lazy {
        Lazy {
            id,
            known: [const { OnceCell::new() }; crate::cx::KNOWN],
            named: [const { OnceCell::new() }; NAMED],
            all: OnceCell::new(),
            guarded: Cell::new(false),
        }
    }

    fn get(&self, name: &str) -> Option<Box<str>> {
        fetched(|out, cap| get_header(self.id, name.as_ptr(), name.len(), out, cap)).map(Into::into)
    }

    pub(crate) fn known(&self, k: crate::cx::Known) -> Option<&str> {
        use crate::cx::Known::*;
        let name = match k {
            IdempotencyKey => "idempotency-key",
            IfNoneMatch => "if-none-match",
            ContentType => "content-type",
            Accept => "accept",
            WispError => crate::protocol::HEADER_ERROR,
            Origin => "origin",
            SecFetchSite => "sec-fetch-site",
        };
        self.known[k as usize]
            .get_or_init(|| self.get(name))
            .as_deref()
    }

    /// Whether the request has read no header but those of `ok`.
    fn reads_only(&self, ok: &[crate::cx::Known]) -> bool {
        self.all.get().is_none()
            && self.named.iter().all(|n| n.get().is_none())
            && (self.known.iter().enumerate())
                .all(|(i, k)| k.get().is_none() || ok.iter().any(|&o| o as usize == i))
    }

    /// Any header but `host`, `content-length` and `transfer-encoding`,
    /// which the wire has.
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        for slot in &self.named {
            let (n, v) = slot.get_or_init(|| (name.into(), self.get(name)));
            if n.eq_ignore_ascii_case(name) {
                return v.as_deref();
            }
        }
        let mut all = self.all().iter();
        all.find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| &**v)
    }

    /// Every header, `host` among them, in the order the host has them.
    pub(crate) fn all(&self) -> &[(Box<str>, Box<str>)] {
        self.all.get_or_init(|| {
            let text = fetched(|out, cap| get_headers(self.id, out, cap)).unwrap_or_default();
            text.lines()
                .filter_map(|l| cut(l, b':'))
                .map(|(n, v)| (n.trim().into(), v.trim().into()))
                .collect()
        })
    }
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
type Handler = fn(u32, Vec<u8>, bool);

/// The task that runs `init`; request ids come from the host and never
/// reach it.
const INIT: u32 = u32::MAX;

static HANDLER: OnceLock<Handler> = OnceLock::new();
/// Starts the task of a connection that has bytes.
static DRIVE: OnceLock<fn(u32, Raw) -> Task> = OnceLock::new();

/// A connection's task is `CONN` and its id.
const CONN: u32 = 1 << 30;
/// What a client may send while its connection waits on the app: more is
/// a flood, and the connection is closed.
const AHEAD: usize = 1 << 20;

/// A connection: idle (its `Raw` here) or being driven (the task has it,
/// and what arrives meanwhile waits in `inbox`).
struct Conn {
    raw: Option<Raw>,
    inbox: Vec<u8>,
}

thread_local! {
    static IN: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static CONNS: RefCell<HashMap<u32, Conn>> = RefCell::new(HashMap::new());
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
    /// The request whose answer is `constant`, until its reply is sent.
    static SAME: Cell<u32> = const { Cell::new(INIT) };
    /// The app has no hook (`before`, `after`, `reroute`) that could change an answer.
    static PLAIN: Cell<bool> = const { Cell::new(false) };
    /// The last peer text and what it parsed to.
    static PEER: RefCell<(String, SocketAddr)> = const { RefCell::new((String::new(), LOCAL)) };
}

const LOCAL: SocketAddr = SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 0);

/// Host export: makes the input buffer at least `len` bytes and returns its address; the host writes its next message there.
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

/// Host export: the `len` bytes in the input buffer are the environment, as `KEY=value` lines separated by NUL.
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

/// Host export: the `len` bytes in the input buffer are request `id`; the app runs it to its reply.
#[unsafe(no_mangle)]
pub extern "C" fn wisp_request(id: u32, len: usize) {
    start_request(id, len, false);
}

/// Host export: like `wisp_request`, with the body read as the host sends it (a stream) rather than all at once.
#[unsafe(no_mangle)]
pub extern "C" fn wisp_request_lazy(id: u32, len: usize) {
    start_request(id, len, true);
}

fn start_request(id: u32, len: usize, lazy: bool) {
    let bytes = take_in(len);
    match HANDLER.get() {
        Some(start) => start(id, bytes, lazy),
        None => spawn(id, Box::pin(finish(id, Reply::plain(500)))),
    }
}

/// Host export: a connection `id` (a WebSocket upgrade) opened; its first `len` bytes are in the input buffer.
#[unsafe(no_mangle)]
pub extern "C" fn wisp_conn_open(id: u32, len: usize) {
    let at = IN.with_borrow(|b| std::str::from_utf8(&b[..len]).map_or(LOCAL, peer));
    CONNS.with_borrow_mut(|c| {
        c.insert(
            id,
            Conn {
                raw: Some(Raw::new(at)),
                inbox: Vec::new(),
            },
        )
    });
}

/// Host export: `len` more bytes from connection `id` are in the input buffer.
#[unsafe(no_mangle)]
pub extern "C" fn wisp_conn_data(id: u32, len: usize) {
    let idle = CONNS.with_borrow_mut(|c| {
        let conn = c.get_mut(&id)?;
        match conn.raw.take() {
            Some(mut raw) => {
                IN.with_borrow(|b| raw.feed(&b[..len]));
                Some(Some(raw))
            }
            None => {
                IN.with_borrow(|b| conn.inbox.extend_from_slice(&b[..len]));
                (conn.inbox.len() <= AHEAD).then_some(None)
            }
        }
    });
    match (idle, DRIVE.get()) {
        (Some(Some(raw)), Some(drive)) => spawn(CONN + id, drive(id, raw)),
        (Some(None), _) => {}
        // A flood, a connection never opened, or an app not started.
        _ => wisp_conn_close(id, true),
    }
}

/// `wisp_conn_close`: the client left. With `end`, the app ends it.
#[unsafe(no_mangle)]
pub extern "C" fn wisp_conn_close(id: u32, end: bool) {
    if CONNS.with_borrow_mut(|c| c.remove(&id)).is_some() && end {
        conn_write(id, std::ptr::null(), 0);
    }
    let task = TASKS.with_borrow_mut(|t| t.remove(&(CONN + id)));
    drop(task); // outside the borrow: dropping it may wake others
    run();
}

/// Host export: the host can take more of connection `id`'s output.
#[unsafe(no_mangle)]
pub extern "C" fn wisp_conn_pull(id: u32) {
    done(&PULLS, CONN + id);
}

/// Sends what `raw` wrote; false when the client is behind.
fn flush(id: u32, raw: &mut Raw) -> bool {
    let w = raw.out();
    let ok = w.is_empty() || conn_write(id, w.as_ptr(), w.len()) != 0;
    raw.sent();
    ok
}

/// The task of connection `id`, with bytes in `raw`: answers them, and what
/// comes while it does, then hands `raw` back to the connection.
async fn connection<A: App>(id: u32, mut raw: Raw) {
    loop {
        Ready.await;
        let step = match READY.get() {
            1 => raw.step::<A>().await,
            _ => raw.refuse::<A>(500),
        };
        flush(id, &mut raw);
        match step {
            Step::Flush => {}
            Step::Idle => {
                let more =
                    CONNS.with_borrow_mut(|c| c.get_mut(&id).map(|c| std::mem::take(&mut c.inbox)));
                match more {
                    Some(more) if more.is_empty() => {
                        raw.rest();
                        CONNS.with_borrow_mut(|c| c.get_mut(&id).map(|c| c.raw = Some(raw)));
                        return;
                    }
                    Some(more) => raw.feed(&more),
                    None => return,
                }
            }
            Step::Close => return wisp_conn_close(id, true),
            Step::Stream(Stream {
                mut body,
                chunked,
                close,
            }) => {
                while let Some(chunk) = body.recv().await {
                    if chunk.is_empty() {
                        continue; // it would read as the end
                    }
                    raw.frame(chunked, &chunk);
                    if !flush(id, &mut raw) {
                        PULLS.with_borrow_mut(|p| p.insert(CONN + id, None));
                        Wait(&PULLS, CONN + id).await;
                    }
                }
                raw.end_stream(chunked);
                flush(id, &mut raw);
                if close {
                    return wisp_conn_close(id, true);
                }
            }
        }
    }
}

/// The task of request `id`: borrows its parts from `bytes`, and answers.
fn request<A: App>(id: u32, bytes: Vec<u8>, lazy: bool) {
    let task = async move {
        let reply = match view(&bytes) {
            Some((method, target, peer, headers, body)) => {
                Ready.await;
                match READY.get() {
                    1 => {
                        let headers = headers.map(|(n, v)| (n, v.as_bytes()));
                        match crate::Cx::from_request::<A>(method, target, headers, body, peer) {
                            Ok(mut cx) => {
                                cx.lazy = lazy.then(|| Lazy::new(id));
                                // The host's cron trigger; out of the way of the routes.
                                if cx.raw_path().starts_with(b"/_wisp/cron/") {
                                    crate::jobs::trigger(&cx).await
                                } else {
                                    crate::http::answer::<A>(cx, &mut None).await
                                }
                            }
                            Err(status) => Reply::plain(status),
                        }
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

/// Host export: the `len` bytes in the input buffer answer the fetch `id` the app asked for.
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

/// Host export: timer `id` fired.
#[unsafe(no_mangle)]
pub extern "C" fn wisp_timer(id: u32) {
    done(&TIMERS, id);
}

/// Host export: the host can take more of the streamed response `id`.
#[unsafe(no_mangle)]
pub extern "C" fn wisp_pull(id: u32) {
    done(&PULLS, id);
}

/// Host export: request `id` was cancelled by the client; its work is dropped.
#[unsafe(no_mangle)]
pub extern "C" fn wisp_cancel(id: u32) {
    let task = TASKS.with_borrow_mut(|t| t.remove(&id));
    drop(task); // outside the borrow: dropping it may wake others
    run();
}

/// Host export: the id of the request or connection the app is working on now.
#[unsafe(no_mangle)]
pub extern "C" fn wisp_current() -> u32 {
    CURRENT.get()
}

/// Host export: runs the app's ready work.
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
    let _ = DRIVE.set(|id, raw| Box::pin(connection::<A>(id, raw)));
    PLAIN.set(!A::BEFORE && !A::AFTER && !A::REROUTE);
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

/// The answer to `cx`, a lazy request, is the same for every request for its
/// path: a baked page or a trailing-slash redirect, made by no hook, from
/// no header but `if-none-match` and `x-wisp-error` (which the bridge
/// looks at itself), with no query and no request id. Its reply is then sent
/// as `200 const`, and the bridge answers the path from what it kept, without
/// entering the wasm.
pub(crate) fn constant(cx: &crate::Cx) {
    use crate::cx::Known::{IfNoneMatch, WispError};
    let Some(lazy) = &cx.lazy else { return };
    if PLAIN.get()
        && !lazy.guarded.get()
        && matches!(cx.method, crate::Method::Get | crate::Method::Head)
        && cx.query_string().is_empty()
        && cx.id().is_none()
        && !crate::settings().request_id
        && lazy.reads_only(&[IfNoneMatch, WispError])
    {
        SAME.set(lazy.id);
    }
}

/// The route has a guard (a rate limit, a middleware: anything run for every
/// request that reads no header), so its answer is not [`constant`].
pub(crate) fn guarded(cx: &crate::Cx) {
    if let Some(lazy) = &cx.lazy {
        lazy.guarded.set(true);
    }
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
        let same = SAME.replace(INIT) == id;
        if matches!(reply.body, crate::Body::Stream(_)) {
            w.extend_from_slice(b" stream");
        } else if same && matches!(reply.status, 200 | 304 | 308) {
            w.extend_from_slice(b" const");
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
