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
//!   before the call that reads them.
//! - `wisp_env(len)`: the environment, `KEY=value` entries ended by NUL.
//! - `wisp_request(id, len)`: a request, `METHOD target peer` then headers,
//!   `name: value` a line each, an empty line and the body.
//! - `wisp_fetched(id, len)`: the answer to import `fetch`, `status`, then as
//!   above (status 0: failed, the body says why).
//!
//! - `wisp_current() -> id`: after a trap, the task that trapped: a
//!   request's id, or `u32::MAX` for `init`.
//! - `wisp_poll()`: after a trap, polls the tasks woken meanwhile.
//!
//! Imports (module `wisp`): `random(ptr, len)`, `log(ptr, len)`, `reply(id, ptr, len)` (the reply to
//! request `id`, as `wisp_fetched` has it), `fetch(id, ptr, len)` (a request,
//! its target a URL).
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
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll, Wake, Waker};

#[link(wasm_import_module = "wisp")]
unsafe extern "C" {
    safe fn random(ptr: *mut u8, len: usize);
    #[link_name = "log"]
    safe fn log_line(ptr: *const u8, len: usize);
    #[link_name = "reply"]
    safe fn send_reply(id: u32, ptr: *const u8, len: usize);
    #[link_name = "fetch"]
    safe fn send_fetch(id: u32, ptr: *const u8, len: usize);
}

type Task = Pin<Box<dyn Future<Output = ()>>>;
type Handler = fn(Request) -> Pin<Box<dyn Future<Output = Reply>>>;

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
    static CURRENT: Cell<u32> = const { Cell::new(INIT) };
    /// `init` has finished: 0 not yet, 1 well, 2 failed.
    static READY: Cell<u8> = const { Cell::new(0) };
    static WAITING: RefCell<Vec<Waker>> = const { RefCell::new(Vec::new()) };
}

#[unsafe(no_mangle)]
pub extern "C" fn wisp_buf(len: usize) -> *mut u8 {
    IN.with_borrow_mut(|b| {
        b.clear();
        b.resize(len, 0);
        b.as_mut_ptr()
    })
}

fn take_in(len: usize) -> Vec<u8> {
    let mut b = IN.take();
    b.truncate(len);
    b
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
    let task = async move {
        let reply = match parse_request(&bytes) {
            Some(req) => {
                Ready.await;
                match READY.get() {
                    1 => {
                        whole(HANDLER.get().expect("wisp::run registers the app")(req).await).await
                    }
                    _ => Reply::plain(500),
                }
            }
            None => Reply::plain(400),
        };
        let wire = encode_reply(&reply);
        send_reply(id, wire.as_ptr(), wire.len());
    };
    spawn(id, Box::pin(task));
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
pub extern "C" fn wisp_current() -> u32 {
    CURRENT.get()
}

#[unsafe(no_mangle)]
pub extern "C" fn wisp_poll() {
    run();
}

/// `wisp::run` in the edge build: remembers the app and starts `init`.
pub(crate) fn start<A: App>() {
    fn handler<A: App>(req: Request) -> Pin<Box<dyn Future<Output = Reply>>> {
        Box::pin(crate::handle::<A>(req))
    }
    log_panics();
    if HANDLER.set(handler::<A>).is_err() {
        return;
    }
    let init = async {
        let ready = match crate::prepare::<A>().await {
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

struct Wakeup(u32);

impl Wake for Wakeup {
    fn wake(self: Arc<Self>) {
        WOKEN.with_borrow_mut(|w| w.push(self.0));
    }
}

fn spawn(id: u32, task: Task) {
    TASKS.with_borrow_mut(|t| t.insert(id, task));
    WOKEN.with_borrow_mut(|w| w.push(id));
    run();
}

/// Polls every woken task until none is. A task is taken out while it is
/// polled, so it may spawn, wake or fetch freely.
fn run() {
    while let Some(id) = WOKEN.with_borrow_mut(Vec::pop) {
        let Some(mut task) = TASKS.with_borrow_mut(|t| t.remove(&id)) else {
            continue;
        };
        let waker = Waker::from(Arc::new(Wakeup(id)));
        CURRENT.set(id);
        if task
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
        {
            TASKS.with_borrow_mut(|t| t.insert(id, task));
        }
    }
}

fn parse_request(bytes: &[u8]) -> Option<Request> {
    let (first, headers, body) = split(bytes)?;
    let mut parts = first.split(' ');
    let (method, target) = (parts.next()?, parts.next()?);
    let peer = parts
        .next()
        .and_then(|p| p.parse().ok())
        .map_or(SocketAddr::from(([127, 0, 0, 1], 0)), |ip| {
            SocketAddr::new(ip, 0)
        });
    let headers = headers
        .map(|(n, v)| (n.to_string(), v.to_string()))
        .collect();
    Some(Request {
        method: method.into(),
        target: target.into(),
        headers,
        body: body.to_vec(),
        peer,
    })
}

/// `first line`, `name: value` lines, an empty line, the body.
fn split(bytes: &[u8]) -> Option<(&str, impl Iterator<Item = (&str, &str)>, &[u8])> {
    let end = bytes.windows(2).position(|w| w == b"\n\n")?;
    let head = std::str::from_utf8(&bytes[..end]).ok()?;
    let (first, rest) = head.split_once('\n').unwrap_or((head, ""));
    let headers = rest
        .lines()
        .filter_map(|l| l.split_once(':'))
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

/// `reply` with a streamed body gathered: edge replies go out whole.
async fn whole(mut reply: Reply) -> Reply {
    if let crate::Body::Stream(rx) = &mut reply.body {
        let mut all = Vec::new();
        while let Some(chunk) = rx.recv().await {
            all.extend_from_slice(&chunk);
        }
        reply.body = crate::Body::Bytes(all);
    }
    reply
}

/// A stream's chunks were already gathered: edge replies go out whole.
fn encode_reply(reply: &Reply) -> Vec<u8> {
    let mut w = format!("{}\n", reply.status).into_bytes();
    push_headers(
        &mut w,
        reply.headers.iter().map(|(n, v)| (&**n, &**v)),
        reply.bytes(),
    );
    w
}
