//! Wisp: a fast, fun web framework for Rust. File-based routes, `.wisp`
//! templates, form actions, one binary. See `docs/design.md` for the whole picture.
//!
//! An app's `main.rs` is `wisp::main!();`; everything else is generated
//! from `src/routes` by `wisp-build`.

// No `unsafe`, but for the edge build's exports and imports (`edge.rs`) and
// the Linux server's io_uring and epoll (`uring.rs`, `epoll.rs`).
#![cfg_attr(
    not(any(target_arch = "wasm32", target_os = "linux")),
    forbid(unsafe_code)
)]
// Every public item says what it is: an editor's hover shows it.
#![deny(missing_docs)]

// The HTML context rules and the live-page wire protocol, which wisp-build
// compiles by too.
use wisp_shared::{contexts, protocol};

mod admin;
mod bake;
mod blob;
mod cache;
#[cfg(not(target_arch = "wasm32"))]
mod channel;
mod compress;
mod content;
mod csp;
mod cx;
mod dev;
#[cfg(target_arch = "wasm32")]
pub mod edge;
#[cfg(target_arch = "wasm32")]
mod edge_store;
mod envconf;
#[cfg(target_os = "linux")]
mod epoll;
mod export;
#[cfg(not(target_arch = "wasm32"))]
mod fetch;
mod form;
#[cfg(test)]
mod fuzz;
mod guard;
mod headers;
mod health;
mod html;
mod http;
mod i18n;
mod idem;
mod image;
#[cfg(feature = "img")]
pub mod img;
mod input;
mod jobs;
pub mod json;
#[cfg(not(target_arch = "wasm32"))]
mod lambda;
mod limit;
mod live;
pub mod oauth;
mod obs;
mod otel;
pub mod password;
mod policy;
mod pwa;
mod range;
#[cfg(not(target_arch = "wasm32"))]
mod relay;
mod remote;
mod rest;
#[doc(hidden)]
pub mod rt_traits;
mod rules;
mod seo;
#[cfg(test)]
mod serve_tests;
mod session;
mod sign;
mod store;
mod swar;
mod table;
mod tail;
#[cfg(not(target_arch = "wasm32"))]
pub mod test;
mod timeout;
mod token;
pub mod totp;
#[cfg(feature = "tower")]
pub mod tower;
#[cfg(feature = "types")]
pub mod ts;
#[cfg(target_os = "linux")]
mod uring;
#[cfg(debug_assertions)]
mod workshop;
mod ws;

pub use blob::{Blobs, Upload, blobs};
pub use cache::{cache, revalidate_tag, uncache};
#[cfg(not(target_arch = "wasm32"))]
pub use channel::{Channel, Subscription, channel};
pub use content::{MdPage, pages};
pub use csp::{csp, csp_off};
pub use cx::{CookieOptions, Cx, Method, SameSite};
#[cfg(target_arch = "wasm32")]
pub use edge::fetch;
pub use export::{Entry, ExportRoute, export, prerender};
#[cfg(not(target_arch = "wasm32"))]
pub use fetch::{fetch, on_fetch};
pub use form::{File, Form};
pub use http::{Body, Reply, Request, TrailingSlash, handle, trailing_slash};
pub use i18n::{default_locale, locales, localize};
pub use image::Image;
pub use input::Email;
pub use jobs::{Queue, cron, queue, work};
pub use json::{FromJson, Value, from_json, to_json};
pub use limit::RateLimit;
pub use live::{ClientModule, Json};
pub use otel::{SpanGuard, span, traceparent};
pub use password::Password;
pub use pwa::app_manifest;
#[cfg(not(target_arch = "wasm32"))]
pub use relay::{Deliver, Relay, relay};
pub use rest::Resource;
pub use seo::og;
pub use session::{Account, login, sign_in_page, sign_out_everywhere, signup, users};
pub use sign::{hex, hmac_sha256};
pub use store::{Changes, Store, store};
pub use table::{Page, Row, Table};
pub use token::{token, untoken};
pub use wisp_macros::{Config, Cookie, FromJson, Json, Rest, action, model, remote};
/// `wisp::based("/x")`: a path of the app's own under its base path (`WISP_BASE`).
pub use wisp_shared::protocol::based;
pub use ws::{Message, WebSocket};

use std::any::{Any, TypeId};
use std::borrow::Cow;
use std::fmt;
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::str::FromStr;
use std::sync::{OnceLock, RwLock};

/// `Result` alone is `Result<()>`: `fn delete(id: u64) -> Result`.
pub type Result<T = (), E = Error> = std::result::Result<T, E>;

/// What every route file, `---` block, `src/hooks.rs` and module of the
/// app's own sees without a `use` line. Other Rust files can
/// `use wisp::prelude::*`.
pub mod prelude {
    pub use crate::RateLimit;
    /// For `wisp::trailing_slash(Always)` in `init`.
    pub use crate::TrailingSlash::{Always, Ignore, Never};
    pub use crate::{
        Config, Cookie, CookieOptions, Cx, Email, Error, FromJson, Image, Json, KB, MB, Method,
        OrStatus, Password, Reply, Response, Rest, Result, Row, SameSite, Shared, Table, Upload,
        Value, action, error, invalid, model, redirect, remote,
    };
}

/// A value every request shares, such as a list kept in memory:
/// `static TODOS: Shared<Vec<String>> = Shared::new(Vec::new());`, then
/// `TODOS.lock().push(text)`. A `Mutex` whose `lock` needs no `unwrap`: a
/// handler that panicked while holding it leaves the value as it was.
pub struct Shared<T>(std::sync::Mutex<T>);

impl<T> Shared<T> {
    /// A shared value, e.g. `static COUNT: Shared<u32> = Shared::new(0);`. Const, so a `static` can hold it.
    pub const fn new(value: T) -> Shared<T> {
        Shared(std::sync::Mutex::new(value))
    }

    /// The value, until the guard is dropped. Do not hold it across an
    /// `.await`: other requests on the thread would wait.
    pub fn lock(&self) -> std::sync::MutexGuard<'_, T> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Sizes for `BODY_LIMIT`: `const BODY_LIMIT: usize = 20 * wisp::MB;`
pub const KB: usize = 1024;
/// A megabyte in bytes, for `#[validate(max_size = 5 * MB)]` and `BODY_LIMIT`.
pub const MB: usize = 1024 * KB;

/// Where a route runs on a host with both (`--target vercel`, `netlify`):
/// `const RUNTIME: wisp::Runtime = wisp::Runtime::Edge;` in its `+page.rs` or
/// `+server.rs`. `wisp build` reads it; other hosts and `cargo run` ignore it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Runtime {
    /// The Node function: full std, files, threads, WebSockets (the default).
    #[default]
    Node,
    /// The edge function: no `std::fs`, thread, process, net or WebSockets.
    Edge,
}

/// The most an `Image` parameter takes unless it has a `#[validate(max_size
/// = …)]` of its own: 2 MB. The route's body limit makes room for it.
pub const MAX_SIZE: usize = 2 * MB;

/// The whole `main.rs` of an app that needs nothing before it starts:
/// [`app!`] plus a `main` that calls [`run`].
#[macro_export]
macro_rules! main {
    () => {
        $crate::app!();

        fn main() {
            $crate::run::<App>();
        }
    };
}

/// What `#[derive(Json)]` adds for `wisp check --types` (the `types`
/// feature): nothing in any other build.
#[doc(hidden)]
#[cfg(feature = "types")]
#[macro_export]
macro_rules! __ts {
    ($($t:tt)*) => { $($t)* };
}

#[doc(hidden)]
#[cfg(not(feature = "types"))]
#[macro_export]
macro_rules! __ts {
    ($($t:tt)*) => {};
}

/// Includes the code `wisp-build` generated and brings `App` into scope,
/// for a `main` of your own: `wisp::app!(); fn main() { setup(); wisp::run::<App>(); }`.
/// `src/hooks.rs` is `crate::hooks`, so routes can use what it defines, and
/// each `src/NAME.rs` this file does not declare is `crate::NAME`.
#[macro_export]
macro_rules! app {
    () => {
        #[doc(hidden)]
        mod __wisp {
            include!(concat!(env!("OUT_DIR"), "/wisp.rs"));
        }
        #[allow(unused_imports)]
        use __wisp::__comps::*;
        #[allow(unused_imports)]
        use __wisp::__mods::*;
        use __wisp::App;
        #[allow(unused_imports)]
        use __wisp::hooks;
    };
}

/// Serves the app on `$HOST:$PORT`. Defaults to port 3000 on 127.0.0.1 in
/// dev (see `WISP_DEV`) and 0.0.0.0 otherwise.
///
/// Runs one worker thread per CPU (`$WISP_THREADS` sets the count) and
/// spreads connections across them. Handlers can use tokio normally. Each
/// worker is a single-threaded tokio runtime, so `tokio::spawn` runs on the
/// handler's own thread.
///
/// On SIGTERM or Ctrl+C it stops taking connections, answers the requests
/// under way (for up to 10 seconds) and returns. A setting that is not
/// valid, or an address it cannot listen on, ends the process with a
/// message saying what to change.
///
/// In the edge build (WebAssembly, for `wisp build --target cloudflare` and
/// the like) the host serves: this gets the app ready and returns.
#[cfg(not(target_arch = "wasm32"))]
pub fn run<A: App>() {
    // `wisp check --types` builds the app to print its scripts' server
    // values' types, and runs nothing else.
    #[cfg(feature = "types")]
    if std::env::var_os("WISP_TYPES").is_some() {
        return print!("{{{}}}", A::types());
    }
    // `wisp build --static` runs the app this way, to write its pages out.
    if let Some(dir) = setting::<String>("WISP_EXPORT", "a folder") {
        let spa = std::env::var_os("WISP_SPA").is_some_and(|v| v == "1");
        let job = export::export::<A>(std::path::Path::new(&dir), spa);
        return export::run(job).unwrap_or_else(|e| fail(&e.to_string()));
    }
    // `wisp build` runs it so for pages with `const PRERENDER: bool = true;`.
    if let Some(dir) = setting::<String>("WISP_PRERENDER", "a folder") {
        let job = export::prerender::<A>(std::path::Path::new(&dir));
        return export::run(job).unwrap_or_else(|e| fail(&e.to_string()));
    }
    // On AWS Lambda (`wisp build --target lambda`), its runtime API hands
    // out the requests.
    if let Some(api) = lambda_api() {
        return lambda::run::<A>(&api);
    }
    let addr = address();
    let threads = match setting("WISP_THREADS", "a number of threads above 0") {
        Some(0) => fail("WISP_THREADS is 0, which is not a number of threads above 0"),
        Some(n) => n,
        None => std::thread::available_parallelism().map_or(1, |n| n.get()),
    };
    if settings().dev && !addr.ip().is_loopback() {
        http::log(format_args!(
            "wisp: dev mode is on (a debug build, or WISP_DEV=on) on {addr}, which other machines reach: \
             error pages show what failed inside\n  Serve a release build (`wisp build`), or set WISP_DEV=off."
        ));
    }
    let served = http::run::<A>(addr, threads);
    store::files::flush();
    obs::flush();
    if let Err(e) = served {
        fail(&e.to_string());
    }
}

/// `AWS_LAMBDA_RUNTIME_API`, which Lambda sets and nothing else does.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn lambda_api() -> Option<String> {
    std::env::var("AWS_LAMBDA_RUNTIME_API")
        .ok()
        .filter(|a| !a.is_empty())
}

/// Starts the app on an edge host (wasm32). The generated `main` calls it.
#[cfg(target_arch = "wasm32")]
pub fn run<A: App>() {
    edge::start::<A>();
}

/// Serves the app on `addr` from inside a tokio runtime you own, for apps
/// that need to set one up themselves. [`run`] is faster, and stops
/// gracefully by itself; this runs until its future is dropped.
#[cfg(not(target_arch = "wasm32"))]
pub async fn serve<A: App>(addr: SocketAddr) -> std::io::Result<()> {
    http::serve::<A>(addr).await
}

/// Gets the app ready to answer requests, for a host other than [`run`] or
/// [`serve`] (which call it): reads the settings, installs what pages and
/// signed cookies need, and runs `init` from `src/hooks.rs`. Call it once,
/// before the first [`handle`]; `wisp::tower::service` and
/// `wisp::test::client` do.
pub async fn prepare<A: App>() -> std::io::Result<()> {
    settings();
    http::setup::<A>();
    A::init().await.map_err(|e| {
        std::io::Error::other(format!("init in src/hooks.rs failed: {}", e.detail()))
    })?;
    session::ready();
    csp::ready(
        A::SCRIPT_HASHES,
        A::PWA.is_some_and(|p| !p.worker.is_empty()),
    );
    content::ready(A::PAGES);
    Ok(())
}

/// Makes `value` available to every request through [`state`]: a database
/// pool, an HTTP client, anything built once. Call it from `init` in
/// `src/hooks.rs` (or from your own `main`, before [`run`]). Providing a
/// second value of the same type replaces the first.
pub fn provide<T: Send + Sync + 'static>(value: T) {
    let value: &'static (dyn Any + Send + Sync) = Box::leak(Box::new(value));
    let mut all = STATE.write().unwrap_or_else(|e| e.into_inner());
    all.retain(|(t, _)| *t != TypeId::of::<T>());
    all.push((TypeId::of::<T>(), value));
    STATE_VERSION.fetch_add(1, std::sync::atomic::Ordering::Release);
}

/// The `T` given to [`provide`]: `wisp::state::<Db>().query(...)`.
///
/// Panics, naming the type, if none was: that is a missing line at startup,
/// found by the first request that needs it.
pub fn state<T: Send + Sync + 'static>() -> &'static T {
    // Each thread reads its own copy of `STATE`, taken again only after a
    // `provide`: a lock here would be written by every thread on every
    // request, and its cache line passed from core to core.
    let version = STATE_VERSION.load(std::sync::atomic::Ordering::Acquire);
    let found = STATE_COPY.with_borrow_mut(|(seen, copy)| {
        if *seen != version {
            copy.clone_from(&STATE.read().unwrap_or_else(|e| e.into_inner()));
            *seen = version;
        }
        copy.iter()
            .find(|(t, _)| *t == TypeId::of::<T>())
            .and_then(|(_, v)| v.downcast_ref::<T>())
    });
    match found {
        Some(v) => v,
        None => panic!(
            "no {} was provided: call wisp::provide(...) with one in `init`, in src/hooks.rs",
            std::any::type_name::<T>()
        ),
    }
}

/// A variable from the host's environment, such as an API key: the
/// process's environment on a server, else `.env` in its working
/// directory (read once, at start); the worker's variables and secrets on
/// an edge host, which has no `.env`.
pub fn env(key: &str) -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    return var(key, dotenv());
    #[cfg(target_arch = "wasm32")]
    return edge::env(key);
}

/// `key` from the process's environment, else from `file`'s lines.
#[cfg(not(target_arch = "wasm32"))]
fn var(key: &str, file: &[(String, String)]) -> Option<String> {
    match std::env::var(key) {
        Ok(v) => Some(v),
        Err(std::env::VarError::NotUnicode(v)) => Some(v.to_string_lossy().into_owned()),
        Err(std::env::VarError::NotPresent) => {
            file.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
        }
    }
}

/// The variables of `.env` in the working directory, read once: the first
/// setting `run` reads reads it. A line that is not `KEY=value` is skipped,
/// with a warning; no file is none.
#[cfg(not(target_arch = "wasm32"))]
fn dotenv() -> &'static [(String, String)] {
    static VARS: OnceLock<Vec<(String, String)>> = OnceLock::new();
    VARS.get_or_init(|| {
        let text = match std::fs::read_to_string(".env") {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(e) => {
                http::log(format_args!("wisp: .env is not read: {e}"));
                return Vec::new();
            }
        };
        let (vars, bad) = wisp_shared::dotenv::parse(&text);
        for n in bad {
            http::log(format_args!(
                "wisp: .env line {n} is not KEY=value, so it is skipped"
            ));
        }
        vars
    })
}

/// Runs `task` in the background: work that outlives its request. On the
/// built-in server it is `tokio::spawn`,
/// on the thread of the request that spawned it (so call it from a request
/// or `init`). In the edge build the host runs it; Cloudflare and Netlify
/// keep the instance alive until its timers and fetches are done.
pub fn spawn(task: impl Future<Output = ()> + Send + 'static) {
    #[cfg(not(target_arch = "wasm32"))]
    tokio::spawn(task);
    #[cfg(target_arch = "wasm32")]
    edge::spawn_task(Box::pin(task));
}

/// Waits for `duration` without holding the thread: `tokio::time::sleep` on
/// the built-in server, the host's `setTimeout` in the edge build.
pub async fn sleep(duration: std::time::Duration) {
    #[cfg(not(target_arch = "wasm32"))]
    tokio::time::sleep(duration).await;
    #[cfg(target_arch = "wasm32")]
    edge::sleep(duration).await;
}

/// Runs `task` every `period`, the first time one `period` from now, until
/// the server stops: a cleanup, a digest email, a cache refresh. Call it
/// from `init` in `src/hooks.rs`:
///
/// ```ignore
/// fn init() {
///     wisp::every(Duration::from_secs(3600), || async {
///         db::delete_expired_sessions().await;
///     });
/// }
/// ```
///
/// Runs in this process, on the thread that called it; a task that takes
/// longer than `period` is not started again until it is done, and one
/// that panics (reported as any panic is) runs again the next period. Not
/// in the edge build, whose instances live for a request: use the host's
/// cron triggers there.
#[cfg(not(target_arch = "wasm32"))]
pub fn every<F, Fut>(period: std::time::Duration, mut task: F)
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    assert!(!period.is_zero(), "wisp::every needs a period above zero");
    spawn(async move {
        let mut next = tokio::time::Instant::now() + period;
        loop {
            let stopped = http::first(
                async {
                    tokio::time::sleep_until(next).await;
                    false
                },
                async {
                    http::stopped().await;
                    true
                },
            )
            .await;
            if stopped {
                return;
            }
            // A panic would end this loop, and with it every later run.
            let mut run = None;
            std::future::poll_fn(|cx| {
                use std::panic::{AssertUnwindSafe, catch_unwind};
                catch_unwind(AssertUnwindSafe(|| {
                    run.get_or_insert_with(|| Box::pin(task()))
                        .as_mut()
                        .poll(cx)
                }))
                .unwrap_or(std::task::Poll::Ready(()))
            })
            .await;
            next = (next + period).max(tokio::time::Instant::now());
        }
    });
}

/// An environment variable parsed as any `FromStr` type, or `default` when
/// it is not set: `let workers: usize = wisp::env_or("WORKERS", 4);`. One
/// that is set but does not parse panics, naming it, so a typo is never
/// quietly replaced by the default (in `init`, that stops the server).
pub fn env_or<T: FromStr>(key: &str, default: T) -> T {
    match env(key) {
        None => default,
        Some(v) => match v.trim().parse() {
            Ok(v) => v,
            Err(_) => panic!(
                "{key} is {v:?}, which is not a {}",
                std::any::type_name::<T>()
            ),
        },
    }
}

/// Whether `a` and `b` are the same, in time that does not depend on where
/// they differ: for checking an API key or a token, which `==` would let an
/// attacker guess a byte at a time.
pub fn secure_eq(a: impl AsRef<[u8]>, b: impl AsRef<[u8]>) -> bool {
    let (a, b) = (a.as_ref(), b.as_ref());
    a.len() == b.len() && a.iter().zip(b).fold(0, |d, (x, y)| d | (x ^ y)) == 0
}

/// Whole seconds since 1970, by the wall clock (the host's, on the edge).
pub(crate) fn unix_now() -> u64 {
    #[cfg(target_arch = "wasm32")]
    return edge::unix_seconds();
    #[cfg(not(target_arch = "wasm32"))]
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The date `days` after 1970-01-01, as (year, month, day): Howard
/// Hinnant's `civil_from_days`.
pub(crate) fn civil(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let (era, doe) = (z / 146_097, z % 146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + u64::from(month <= 2), month, day)
}

/// `n` in decimal, written so it ends at `buf[end]`: returns where it
/// starts. Two digits at a time from a table, so a division per pair
/// rather than per digit.
pub(crate) fn digits(buf: &mut [u8], end: usize, mut n: u64) -> usize {
    const PAIRS: &[u8; 200] = b"\
        0001020304050607080910111213141516171819\
        2021222324252627282930313233343536373839\
        4041424344454647484950515253545556575859\
        6061626364656667686970717273747576777879\
        8081828384858687888990919293949596979899";
    let mut i = end;
    while n >= 100 {
        let pair = (n % 100) as usize * 2;
        n /= 100;
        i -= 2;
        buf[i..i + 2].copy_from_slice(&PAIRS[pair..pair + 2]);
    }
    if n >= 10 {
        let pair = n as usize * 2;
        i -= 2;
        buf[i..i + 2].copy_from_slice(&PAIRS[pair..pair + 2]);
    } else {
        i -= 1;
        buf[i] = b'0' + n as u8;
    }
    i
}

/// `n` in decimal onto `out`: what templates and JSON write integers with.
pub(crate) fn decimal(out: &mut String, n: u64) {
    let mut buf = [0u8; 20];
    let start = digits(&mut buf, 20, n);
    out.push_str(std::str::from_utf8(&buf[start..]).unwrap_or_default());
}

/// Values given to [`provide`], leaked: they live as long as the process.
static STATE: RwLock<Vec<Provided>> = RwLock::new(Vec::new());
type Provided = (TypeId, &'static (dyn Any + Send + Sync));
/// How many times [`provide`] has run: a thread whose copy of `STATE` is
/// older takes it again.
static STATE_VERSION: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

thread_local! {
    /// This thread's copy of `STATE`, and the version it is.
    static STATE_COPY: std::cell::RefCell<(usize, Vec<Provided>)> =
        const { std::cell::RefCell::new((0, Vec::new())) };
}

/// `$HOST:$PORT`, with the defaults described in [`run`]. `HOST` is an IP
/// address or a name such as `localhost`.
/// Ends the process with a message if either is not valid.
pub fn address() -> SocketAddr {
    let port: u16 = setting("PORT", "a port number from 0 to 65535").unwrap_or(3000);
    let Some(host) = setting::<String>("HOST", "an address") else {
        let ip = if settings().dev {
            Ipv4Addr::LOCALHOST
        } else {
            Ipv4Addr::UNSPECIFIED
        };
        return SocketAddr::from((ip, port));
    };
    let bare = host.trim().trim_start_matches('[').trim_end_matches(']');
    match (bare, port)
        .to_socket_addrs()
        .ok()
        .and_then(|mut all| all.next())
    {
        Some(addr) => addr,
        None => fail(&format!(
            "HOST is {host:?}, which is not an IP address or a name that resolves to one\n  \
             Use 127.0.0.1 (this machine only), 0.0.0.0 (every IPv4 address), :: (every IPv6 one) or a name such as localhost."
        )),
    }
}

/// An environment setting: `None` when it is not set. One that is set but
/// is not a `T` ends the process, so a typo is never silently replaced by a
/// default.
fn setting<T: FromStr>(name: &str, what: &str) -> Option<T> {
    let value = env(name)?;
    match value.trim().parse() {
        Ok(v) => Some(v),
        _ => fail(&format!("{name} is {value:?}, which is not {what}")),
    }
}

/// Stops a server that cannot start, with a message (the reason, then what
/// to do indented under it).
fn fail(message: &str) -> ! {
    http::log(format_args!("wisp: {message}"));
    std::process::exit(1);
}

/// Settings read from the environment, once. `run` and `serve` read them
/// before serving, so a bad one stops the server at start.
pub(crate) struct Settings {
    /// `WISP_DEV`: dev mode (`on` in debug builds, `off` in release ones):
    /// 5xx details on error pages, files read from `static/` on each
    /// request, the dev log and tools, a dev secret for signed cookies.
    pub dev: bool,
    /// `WISP_BODY_LIMIT`: the largest request body a route takes unless it
    /// sets its own `BODY_LIMIT`.
    pub body_limit: usize,
    /// `ORIGIN`: the site's own address, as browsers see it
    /// (`https://example.com`), for a proxy that does not pass `Host` on.
    pub origin: Option<String>,
    /// `WISP_CLIENT_IP_HEADER`: where the proxy puts the client's address.
    pub client_ip_header: Option<String>,
    /// `WISP_SECRET`: signs cookies.
    pub secret: Option<String>,
    /// `WISP_SECRET_OLD`: the secret before, still accepted on cookies it
    /// signed while a new one takes over.
    pub old_secret: Option<String>,
    /// `WISP_WS_IDLE`: seconds a WebSocket client may stay quiet (60; 0
    /// never closes). It is pinged halfway.
    #[cfg(not(target_arch = "wasm32"))] // no upgrades there
    pub ws_idle: std::time::Duration,
    /// `WISP_MAX_CONNS`: open connections, WebSockets too, past which the
    /// built-in server answers new ones 503 and closes them (10000; 0 is
    /// no cap).
    #[cfg(not(target_arch = "wasm32"))] // no sockets there
    pub max_conns: usize,
    /// `WISP_API_DOCS`: serve `/_wisp/openapi.json` and `/_wisp/docs`
    /// (`on` in dev, `off` otherwise).
    pub api_docs: bool,
    /// `WISP_REQUEST_ID`: give every request an id, not only those that ask
    /// for one with `cx.request_id()` (`off`).
    pub request_id: bool,
    /// `WISP_SECURE_HEADERS`: `nosniff` and `referrer-policy` on pages (`on`).
    pub secure_headers: bool,
    /// Handlers are timed, for the dev log: `dev`, but not in the edge build.
    pub timed: bool,
    /// `WISP_HANDLER_TIMEOUT`, in milliseconds (0 for none).
    pub timeout_ms: u64,
    /// `WISP_PROBLEM_JSON`: JSON errors as RFC 9457 `application/problem+json`
    /// for every client, not only those whose `accept` asks for it (`off`).
    pub problem_json: bool,
}

/// An `on`/`off` setting, `default` when it is not set.
fn switch(name: &str, default: bool) -> bool {
    match setting::<String>(name, "on or off").map(|v| v.to_ascii_lowercase()) {
        None => default,
        Some(v) if matches!(&*v, "on" | "1" | "true") => true,
        Some(v) if !input::on(&v) => false,
        Some(v) => fail(&format!("{name} is {v:?}, which is not on or off")),
    }
}

pub(crate) fn settings() -> &'static Settings {
    static SETTINGS: OnceLock<Settings> = OnceLock::new();
    SETTINGS.get_or_init(|| {
        let body_limit = setting::<Size>("WISP_BODY_LIMIT", "a size such as 1048576, 512KB or 10MB").map_or(MB, |s| s.0);
        let origin = setting::<String>("ORIGIN", "an address").map(|o| {
            let o = o.trim_end_matches('/').to_ascii_lowercase();
            let host = o.strip_prefix("https://").or_else(|| o.strip_prefix("http://"));
            if host.is_none_or(|h| h.is_empty() || h.contains(['/', '?', '#', '@', ' '])) {
                fail(&format!("ORIGIN is {o:?}, which is not a site's address\n  Use the address visitors type, such as https://example.com, with no path."));
            }
            o
        });
        let client_ip_header = setting::<String>("WISP_CLIENT_IP_HEADER", "a header name").map(|h| h.to_ascii_lowercase());
        let secret = |name: &str| {
            let s = setting::<String>(name, "a secret")?;
            if s.len() < 32 {
                fail(&format!(
                    "{name} is {} characters, too short to keep signed cookies safe\n  Use at least 32 random ones: `openssl rand -hex 32` makes some.",
                    s.len()
                ));
            }
            Some(s)
        };
        let (secret, old_secret) = (secret("WISP_SECRET"), secret("WISP_SECRET_OLD"));
        #[cfg(not(target_arch = "wasm32"))]
        let ws_idle = std::time::Duration::from_secs(setting::<u64>("WISP_WS_IDLE", "a number of seconds").unwrap_or(60));
        #[cfg(not(target_arch = "wasm32"))]
        let max_conns = match setting::<usize>("WISP_MAX_CONNS", "a number of connections") {
            Some(0) => usize::MAX,
            n => n.unwrap_or(10_000),
        };
        let dev = switch("WISP_DEV", cfg!(debug_assertions));
        let api_docs = switch("WISP_API_DOCS", dev);
        let request_id = switch("WISP_REQUEST_ID", false);
        let problem_json = switch("WISP_PROBLEM_JSON", false);
        let secure_headers = switch("WISP_SECURE_HEADERS", true);
        // `WISP_HSTS`: `strict-transport-security` on every answer (`off`).
        headers::HSTS_ON.store(switch("WISP_HSTS", false), std::sync::atomic::Ordering::Relaxed);
        let timed = cfg!(not(target_arch = "wasm32")) && dev;
        let timeout_ms = setting::<u64>("WISP_HANDLER_TIMEOUT", "a number of seconds").map_or(0, |s| s.saturating_mul(1000));
        Settings {
            dev, body_limit, origin, client_ip_header, secret, old_secret, api_docs, request_id, problem_json, secure_headers, timed, timeout_ms,
            #[cfg(not(target_arch = "wasm32"))]
            ws_idle,
            #[cfg(not(target_arch = "wasm32"))]
            max_conns,
        }
    })
}

/// A byte count, written `1048576`, `512KB`, `10MB` or `1GB` (powers of 1024).
struct Size(usize);

impl FromStr for Size {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Size, ()> {
        let s = s.trim().to_ascii_uppercase();
        let (digits, unit) = [("GB", 1 << 30), ("MB", MB), ("KB", KB), ("B", 1)]
            .into_iter()
            .find_map(|(suffix, unit)| Some((s.strip_suffix(suffix)?, unit)))
            .unwrap_or((&s, 1));
        digits
            .trim()
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_mul(unit))
            .map(Size)
            .ok_or(())
    }
}

/// Implemented by the code `wisp-build` generates. Not meant to be written
/// by hand; the methods are static so the whole server is monomorphized
/// around one app with no dynamic dispatch.
pub trait App: 'static {
    /// Project directory (dev builds read templates, CSS and static files here).
    const ROOT: &'static str;
    /// Hash of the built CSS, `"dev"` in dev builds, `None` without CSS.
    const CSS: Option<&'static str>;
    /// `'sha256-…'` of every inline script the templates and shell run,
    /// which the Content-Security-Policy allows (see `csp`).
    const SCRIPT_HASHES: &'static [&'static str] = &[];
    /// The Markdown pages, for [`pages`].
    const PAGES: &'static [MdPage] = &[];
    /// The tags of the components built as custom elements (`{@element
    /// "x-card"}`), which other sites load: with any, the browser modules
    /// allow any origin.
    const ELEMENTS: &'static [&'static str] = &[];
    /// The service worker and web app manifest, if the app has either.
    const PWA: Option<rt::Pwa> = None;
    /// What the build knows of each route, by route id.
    const ROUTES: &'static [rt::RouteFacts];
    /// [`rt::RouteFacts::now`] for a request no route matched, which the
    /// root error page answers.
    const NOT_FOUND_NOW: bool = false;
    /// The app has no page, only `+server.rs` endpoints: every error is JSON.
    const API_ONLY: bool = false;
    /// The first path segments under which there are `+server.rs` endpoints
    /// and nothing else: an unmatched path there is an endpoint's error.
    const API_PREFIXES: &'static [&'static str] = &[];
    /// The app may call [`trailing_slash`]: without it, a page's address
    /// is never redirected to end in `/`, and no request looks.
    const TRAILING_SLASH: bool = true;
    /// `src/hooks.rs` has `before`. Nothing in the server reads it; the edge
    /// build (`edge.rs`) does, since a hook can change any answer.
    const BEFORE: bool = false;
    /// `src/hooks.rs` has `after`: [`App::after`] runs on every reply.
    const AFTER: bool = false;
    /// `src/hooks.rs` has `report`: [`App::report`] runs on every 5xx.
    const REPORT: bool = false;
    /// The locales of `src/locales/*.json`, by file name, sorted.
    const LOCALES: &'static [&'static str] = &[];
    /// `(path, shape)` per template id, for dev hot swapping.
    const TEMPLATES: &'static [(&'static str, u64)];

    /// The route of `path` and its parameters, slices of it.
    fn route(path: &str) -> Option<(usize, [&str; cx::MAX_PARAMS])>;
    /// The page shell split around `%wisp.head%` and `%wisp.body%`: head start, between, end.
    fn shell() -> [&'static str; 3];
    /// The embedded file served at `path`, if any.
    fn asset(path: &str) -> Option<&'static Asset>;
    /// The browser module of a template with client code, by its path
    /// (`/_app/c/t3.js`).
    fn client_module(path: &str) -> Option<&'static ClientModule> {
        let _ = path;
        None
    }
    /// The OpenAPI 3.1 document of the app's `+server.rs` endpoints, pages
    /// and form actions, served at `/_wisp/openapi.json` (`wisp openapi`
    /// prints it); empty without endpoints and actions.
    fn openapi() -> &'static str {
        ""
    }
    /// A typed TypeScript client of the same endpoints, served at
    /// `/_wisp/client.ts`; empty without any.
    fn client_ts() -> &'static str {
        ""
    }
    /// Dev builds: the components and their stories, for the workshop at
    /// `/_wisp/components`.
    fn workshop() -> &'static [rt::Shelf] {
        &[]
    }
    /// `wisp check --types`: the TypeScript of each value a script reads
    /// of a block, by file (see `ts`), as the members of a JSON object.
    #[cfg(feature = "types")]
    fn types() -> String {
        String::new()
    }
    /// Every route, for `wisp build --static`.
    fn export_routes() -> Vec<ExportRoute> {
        Vec::new()
    }
    /// `init` from `src/hooks.rs`, run once before the server listens.
    fn init() -> impl Future<Output = Result<()>>;
    /// Every request that is not a static file: `before` from
    /// `src/hooks.rs`, then the route (`None` is a 404).
    fn handle(
        route: Option<usize>,
        cx: &mut Cx,
        out: &mut Out,
    ) -> impl Future<Output = Result<()>> + Send;
    /// Renders the error page for `status` and `message`, from `+error.wisp` of `route` or the nearest above.
    fn error(
        route: Option<usize>,
        cx: &mut Cx,
        out: &mut Out,
        status: u16,
        message: &str,
    ) -> impl Future<Output = Result<()>> + Send;
    /// `after` from `src/hooks.rs`: the reply, before it is sent.
    fn after(cx: &mut Cx, reply: &mut Reply) {
        let _ = (cx, reply);
    }
    /// `report` from `src/hooks.rs`: the error of a 5xx, whole.
    fn report(cx: &mut Cx, err: &Error) {
        let _ = (cx, err);
    }
    /// `redirects` in `[package.metadata.wisp]`: [`App::redirect`] looks at
    /// every request first.
    const REDIRECTS: bool = false;
    /// `rewrites` there: [`App::rewrite`] routes a path no route matches.
    const REWRITES: bool = false;
    /// `headers` there: [`App::headers`] sets them on every reply.
    const HEADERS: bool = false;
    /// The redirect of the request's path, if a rule has one. Never called
    /// without `REDIRECTS`.
    fn redirect(cx: &Cx, reply: &mut Reply) -> bool {
        let _ = (cx, reply);
        false
    }
    /// The route a rule sends `path` to, and its parameters.
    fn rewrite(path: &str) -> Option<(usize, [&str; cx::MAX_PARAMS])> {
        let _ = path;
        None
    }
    /// The rules' headers for the request's path.
    fn headers(cx: &Cx, reply: &mut Reply) {
        let _ = (cx, reply);
    }
    /// `src/hooks.rs` has `reroute`: [`App::reroute`] sees every path first.
    const REROUTE: bool = false;
    /// `reroute` from `src/hooks.rs`: the path to look a route up by, a part
    /// of `path` (or one with no parameters). Never called without it.
    fn reroute(path: &str) -> &str {
        path
    }
    /// What [`App::handle`] does, for the arms the build made plain code
    /// ([`rt::RouteFacts::sync`]): `Ok(false)` for any other, having done
    /// nothing.
    fn handle_now(route: Option<usize>, cx: &mut Cx, out: &mut Out) -> Result<bool> {
        let _ = (route, cx, out);
        Ok(false)
    }
}

/// Where a request's output goes: HTML for the head and body of the shell,
/// or a complete response from a `+server.rs` endpoint.
#[derive(Default)]
pub struct Out {
    /// HTML for the document's `<head>`.
    pub head: String,
    /// HTML for the document's `<body>`.
    pub body: String,
    response: Option<Response>,
    /// A response made before, sent as its bytes: a baked page, or one
    /// `CACHE` kept.
    made: Option<bake::Made>,
    /// The template instances with browser code the page rendered.
    live: live::Live,
    /// The request's locale, by index into [`locales`]: what `t(…)` in a
    /// template reads.
    #[doc(hidden)]
    pub lang: u8,
    /// The request, while logs, metrics or traces watch it.
    obs: Option<obs::Pending>,
}

impl Out {
    fn clear(&mut self) {
        self.head.clear();
        self.body.clear();
        self.response = None;
        self.made = None;
        self.live.clear();
        self.lang = 0;
    }
}

/// A file embedded in a release binary.
pub struct Asset {
    /// The file's bytes.
    pub body: &'static [u8],
    /// The file extension, which sets its content type.
    pub ext: &'static str,
    /// Its ETag, for `304` answers.
    pub etag: &'static str,
}

/// A complete response: from a `+server.rs` endpoint, or from an action or
/// the `before` hook, in place of the page.
pub struct Response {
    /// The HTTP status code.
    pub status: u16,
    /// The `Content-Type` header.
    pub content_type: Cow<'static, str>,
    /// Other headers, as `(name, value)`.
    pub headers: Vec<(Cow<'static, str>, String)>,
    /// The body.
    pub body: Vec<u8>,
    /// For [`Response::stream`]: the body, as it is made.
    stream: Option<tokio::sync::mpsc::Receiver<Vec<u8>>>,
    /// For [`Response::websocket`]: what runs once upgraded.
    upgrade: Option<ws::Upgrade>,
    /// Made by [`Response::html`]: it gets the page's security headers.
    pub(crate) page: bool,
}

impl Response {
    /// A `200` with `content_type` and `body`: `Response::new("image/png", bytes)`.
    pub fn new(content_type: impl Into<Cow<'static, str>>, body: impl Into<Vec<u8>>) -> Response {
        Response {
            status: 200,
            content_type: content_type.into(),
            headers: Vec::new(),
            body: body.into(),
            stream: None,
            upgrade: None,
            page: false,
        }
    }

    /// A response whose body is sent while it is being made: a live feed,
    /// a large export. `body` writes it, in a task of its own:
    ///
    /// ```ignore
    /// Response::stream("text/csv", |out| async move {
    ///     for row in rows().await {
    ///         out.send(row.to_csv()).await?; // stops once the client has left
    ///     }
    ///     Ok(())
    /// })
    /// ```
    ///
    /// Each `send` goes out at once; the body ends when `body` returns, or
    /// when the server stops.
    pub fn stream<F, Fut>(content_type: impl Into<Cow<'static, str>>, body: F) -> Response
    where
        F: FnOnce(Sender) -> Fut,
        Fut: Future<Output = Result<(), Gone>> + Send + 'static,
    {
        let (res, tx) = Response::channel(content_type);
        let task = body(tx);
        spawn(async move {
            let _ = task.await; // `Gone`: the client left, which ends it too
        });
        res
    }

    /// Newline-delimited JSON (`application/x-ndjson`), a value per line,
    /// sent as it is made: a big list without holding all of it.
    ///
    /// ```ignore
    /// fn get() -> Response {
    ///     Response::ndjson(|out| async move {
    ///         for note in db::notes().await {
    ///             out.line(&note).await?;
    ///         }
    ///         Ok(())
    ///     })
    /// }
    /// ```
    pub fn ndjson<F, Fut>(body: F) -> Response
    where
        F: FnOnce(Sender) -> Fut,
        Fut: Future<Output = Result<(), Gone>> + Send + 'static,
    {
        Response::stream("application/x-ndjson", body)
    }

    /// A streamed response, and the [`Sender`] that writes its body until
    /// it is dropped.
    pub(crate) fn channel(content_type: impl Into<Cow<'static, str>>) -> (Response, Sender) {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let mut res = Response::new(content_type, Vec::new());
        res.stream = Some(rx);
        (res, Sender(tx))
    }

    /// No body, only a status (and the headers added to it):
    /// `Response::empty(204)`.
    pub fn empty(status: u16) -> Response {
        Response::new("", Vec::new()).with_status(status)
    }

    /// `body` as a file the browser saves as `name`, typed by its extension:
    /// `Response::download("report.csv", csv)`.
    pub fn download(name: &str, body: impl Into<Vec<u8>>) -> Response {
        let ext = name
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        // The name goes in a quoted header value: nothing in it may end the quotes or the line.
        let safe: String = name
            .chars()
            .map(|c| {
                if c == '"' || c == '\\' || c.is_control() {
                    '_'
                } else {
                    c
                }
            })
            .collect();
        Response::new(http::mime(&ext), body).with_header(
            "content-disposition",
            format!("attachment; filename=\"{safe}\""),
        )
    }

    /// The file `name` in the directory `dir`, such as an upload saved
    /// earlier, typed by its extension:
    /// `Response::file_in("uploads", cx.param("name")).await?`.
    ///
    /// `name` may come from the URL: one that would reach outside `dir`
    /// (`..`, an absolute path, a drive) is a 404, as is a file that does not
    /// exist. It is read without blocking the thread.
    pub async fn file_in(dir: impl AsRef<std::path::Path>, name: &str) -> Result<Response> {
        if !http::stays_inside(name) {
            return Err(Error::new(404, "Not Found"));
        }
        let path = dir.as_ref().join(name);
        // Edge hosts have no files: there it is the 500 of an unsupported
        // operation.
        #[cfg(not(target_arch = "wasm32"))]
        let read = tokio::fs::read(&path).await;
        #[cfg(target_arch = "wasm32")]
        let read = std::fs::read(&path);
        match read {
            Ok(body) => {
                let ext = path
                    .extension()
                    .map(|e| e.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();
                Ok(Response::new(http::mime(&ext), body))
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound
                        | std::io::ErrorKind::IsADirectory
                        | std::io::ErrorKind::PermissionDenied
                ) =>
            {
                Err(Error::new(404, "Not Found"))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Server-sent events, which a page receives with `listen(url, …)` or
    /// `new EventSource(url)`: a stream that no proxy or browser caches or
    /// holds back. `body` sends them, each with [`Sender::event`], in a task
    /// of its own:
    ///
    /// ```ignore
    /// Response::events(|out| async move {
    ///     loop {
    ///         out.event(&now()).await?; // stops once the client has left
    ///         wisp::sleep(Duration::from_secs(1)).await;
    ///     }
    /// })
    /// ```
    pub fn events<F, Fut>(body: F) -> Response
    where
        F: FnOnce(Sender) -> Fut,
        Fut: Future<Output = Result<(), Gone>> + Send + 'static,
    {
        // `x-accel-buffering` stops nginx from holding events back.
        Response::stream("text/event-stream", body)
            .with_header("cache-control", "no-store")
            .with_header("x-accel-buffering", "no")
    }

    /// A `200` with `text/plain; charset=utf-8`: `Response::text("ok")`.
    pub fn text(body: impl IntoText) -> Response {
        Response::new("text/plain; charset=utf-8", body.into_text())
    }

    /// A `200` with `text/html; charset=utf-8`: `Response::html("<p>hi</p>")`. Not escaped.
    pub fn html(body: impl IntoText) -> Response {
        let mut res = Response::new("text/html; charset=utf-8", body.into_text());
        res.page = true;
        res
    }

    /// Serialize with whatever you like; this only sets the content type.
    pub fn json(body: impl IntoText) -> Response {
        Response::new("application/json", body.into_text())
    }

    /// `value` as JSON, with `#[derive(Json)]` or one of the built-in impls.
    pub fn json_of(value: &(impl Json + ?Sized)) -> Response {
        let mut out = String::from_utf8(http::spare()).unwrap_or_default();
        value.json(&mut out);
        Response::json(out)
    }

    /// `value` as JSON with 201 Created, the answer to a POST that made
    /// something: `Response::created(&note)`.
    pub fn created(value: &(impl Json + ?Sized)) -> Response {
        Response::json_of(value).with_status(201)
    }

    /// The same response with `status` (100 to 999; panics outside it): `Response::text("gone").with_status(410)`.
    pub fn with_status(mut self, status: u16) -> Response {
        assert!((100..=999).contains(&status), "invalid status {status}");
        self.status = status;
        self
    }

    /// Panics on CR/LF in the value (header injection).
    pub fn with_header(
        mut self,
        name: impl Into<Cow<'static, str>>,
        value: impl Into<String>,
    ) -> Response {
        let (name, value) = (name.into(), value.into());
        assert!(
            cx::valid_header(&name, &value),
            "invalid header {name:?}: {value:?}"
        );
        self.headers.push((name, value));
        self
    }
}

/// The text of [`Response::text`], [`Response::html`] and
/// [`Response::json`]. A `String` becomes the body as it is; borrowed text
/// is copied into a buffer that an earlier response on the thread left, so
/// a warm server allocates nothing for it.
#[diagnostic::on_unimplemented(
    message = "a response's text is a `String` or a `&str`, not `{Self}`"
)]
pub trait IntoText {
    /// The bytes of the body.
    fn into_text(self) -> Vec<u8>;
}

impl IntoText for String {
    fn into_text(self) -> Vec<u8> {
        self.into_bytes()
    }
}

impl IntoText for &str {
    fn into_text(self) -> Vec<u8> {
        let mut body = http::spare();
        body.extend_from_slice(self.as_bytes());
        body
    }
}

impl IntoText for &mut str {
    fn into_text(self) -> Vec<u8> {
        (&*self).into_text()
    }
}

impl IntoText for &String {
    fn into_text(self) -> Vec<u8> {
        self.as_str().into_text()
    }
}

impl IntoText for Box<str> {
    fn into_text(self) -> Vec<u8> {
        self.into_string().into_bytes()
    }
}

impl IntoText for Cow<'_, str> {
    fn into_text(self) -> Vec<u8> {
        match self {
            Cow::Owned(s) => s.into_bytes(),
            Cow::Borrowed(s) => s.into_text(),
        }
    }
}

impl IntoText for char {
    fn into_text(self) -> Vec<u8> {
        self.encode_utf8(&mut [0; 4]).into_text()
    }
}

/// Writes the body of a [`Response::stream`].
pub struct Sender(tokio::sync::mpsc::Sender<Vec<u8>>);

impl Sender {
    /// Sends `chunk` to the client. Fails once the client has gone (or the
    /// server is stopping): stop making the body then.
    pub async fn send(&self, chunk: impl Into<Vec<u8>>) -> Result<(), Gone> {
        self.0.send(chunk.into()).await.map_err(|_| Gone)
    }

    /// Sends one server-sent event, which the page's `EventSource` gets as a
    /// `message` whose `data` is `data`. Each line of it is sent as its own
    /// `data:` field, as the format requires, so any text arrives whole.
    pub async fn event(&self, data: &str) -> Result<(), Gone> {
        self.send(event_text(data)).await
    }

    /// Sends `value` as JSON and a newline: a line of [`Response::ndjson`].
    pub async fn line(&self, value: &(impl Json + ?Sized)) -> Result<(), Gone> {
        let mut out = String::with_capacity(64);
        value.json(&mut out);
        out.push('\n');
        self.send(out).await
    }

    /// Whether the client has gone, so nothing more can reach it.
    pub fn is_closed(&self) -> bool {
        self.0.is_closed()
    }
}

/// `data` as one server-sent event: a `data:` field per line. A line ends
/// at CR LF, LF or a CR alone, as the browser reads it, so text with a CR
/// in it cannot start a field of its own (`event:`, `id:`, `retry:`).
fn event_text(data: &str) -> String {
    let mut out = String::with_capacity(data.len() + 8);
    let mut rest = data;
    loop {
        let end = rest.find(['\r', '\n']).unwrap_or(rest.len());
        out.push_str("data: ");
        out.push_str(&rest[..end]);
        out.push('\n');
        if end == rest.len() {
            break;
        }
        let eol = if rest[end..].starts_with("\r\n") {
            2
        } else {
            1
        };
        rest = &rest[end + eol..];
    }
    out.push('\n');
    out
}

/// The client of a streamed response has gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gone;

impl fmt::Display for Gone {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("the client has gone")
    }
}

impl std::error::Error for Gone {}

/// An HTTP error or redirect. `?` converts any `std::error::Error` into a
/// 500 whose details are shown only in dev builds.
pub struct Error {
    status: u16,
    message: Cow<'static, str>,
    /// `location` for redirects, `allow` for 405, `retry-after` for 429.
    header: Option<Box<(&'static str, String)>>,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
    /// For a 422: what is wrong, by field.
    fields: Vec<(String, String)>,
    /// What an API client matches on, from [`Error::with_code`]; else one
    /// for the status.
    code: Option<&'static str>,
}

impl Error {
    /// An error with an HTTP `status` and a `message` shown to the visitor: `Error::new(403, "not yours")`.
    pub fn new(status: u16, message: impl Into<Cow<'static, str>>) -> Error {
        assert!(
            (400..=599).contains(&status),
            "error status must be 4xx or 5xx, got {status}"
        );
        Error::raw(status, message.into())
    }

    /// An error of any status, a redirect's too: [`Error::new`] checks it.
    fn raw(status: u16, message: Cow<'static, str>) -> Error {
        Error {
            status,
            message,
            header: None,
            source: None,
            fields: Vec::new(),
            code: None,
        }
    }

    /// A code for API clients to match on, in place of the status's own
    /// (`not_found`, `invalid`...): `Error::new(409, "Taken").with_code("email_taken")`.
    pub fn with_code(mut self, code: &'static str) -> Error {
        self.code = Some(code);
        self
    }

    /// The error's code: the one given, or the status's: `bad_request`
    /// `unauthorized` `forbidden` `not_found` `method_not_allowed`
    /// `conflict` `precondition_failed` `too_large` `unsupported_media_type`
    /// `invalid` `rate_limited` `internal` `unavailable`...
    pub fn code(&self) -> &'static str {
        self.code.unwrap_or(match self.status {
            400 => "bad_request",
            401 => "unauthorized",
            403 => "forbidden",
            404 => "not_found",
            405 => "method_not_allowed",
            406 => "not_acceptable",
            408 | 504 => "timeout",
            409 => "conflict",
            410 => "gone",
            412 => "precondition_failed",
            413 => "too_large",
            415 => "unsupported_media_type",
            422 => "invalid",
            428 => "precondition_required",
            429 => "rate_limited",
            431 => "headers_too_large",
            500 => "internal",
            501 => "not_implemented",
            502 => "bad_gateway",
            503 => "unavailable",
            s if s < 500 => "client_error",
            _ => "server_error",
        })
    }

    /// A 422 for input that does not pass: `field` and what is wrong with
    /// it. [`invalid`] returns one; [`Error::and`] adds more.
    pub fn invalid(field: impl Into<String>, problem: impl Into<String>) -> Error {
        Error::invalid_fields(vec![(field.into(), problem.into())])
    }

    /// Another field that does not pass, on a 422 from [`Error::invalid`].
    pub fn and(mut self, field: impl Into<String>, problem: impl Into<String>) -> Error {
        self.fields.push((field.into(), problem.into()));
        self.message = Cow::Owned(Error::summary(&self.fields));
        self
    }

    pub(crate) fn invalid_fields(fields: Vec<(String, String)>) -> Error {
        Error {
            message: Cow::Owned(Error::summary(&fields)),
            fields,
            ..Error::new(422, "")
        }
    }

    /// `title: must have at least 1 character; age: is required`.
    fn summary(fields: &[(String, String)]) -> String {
        let all: Vec<String> = fields.iter().map(|(f, p)| format!("{f}: {p}")).collect();
        all.join("; ")
    }

    /// What is wrong, by field, for a 422 about input: `("title", "is required")`.
    pub fn fields(&self) -> &[(String, String)] {
        &self.fields
    }

    /// A header to send with the error, such as `retry-after` on a 429.
    /// Panics on CR/LF in the value.
    pub fn with_header(mut self, name: &'static str, value: impl Into<String>) -> Error {
        let value = value.into();
        assert!(
            cx::valid_header(name, &value),
            "invalid header {name:?}: {value:?}"
        );
        self.header = Some(Box::new((name, value)));
        self
    }

    /// The error as JSON, for an API client: `{"status":422,"code":"invalid",
    /// "error":"...","errors":{"title":"is required"}}` (`errors` only when
    /// there are some). As a `problem` (RFC 9457) it is `{"type":"about:blank",
    /// "title":"Unprocessable Content","status":422,"detail":"...",…}`.
    pub(crate) fn json(&self, message: &str, problem: bool) -> String {
        let mut out = String::with_capacity(96);
        if problem {
            out.push_str("{\"type\":\"about:blank\",\"title\":");
            http::reason(self.status).json(&mut out);
            out.push(',');
        } else {
            out.push('{');
        }
        out.push_str("\"status\":");
        self.status.json(&mut out);
        out.push_str(",\"code\":");
        self.code().json(&mut out);
        out.push_str(if problem {
            ",\"detail\":"
        } else {
            ",\"error\":"
        });
        message.json(&mut out);
        if !self.fields.is_empty() {
            out.push_str(",\"errors\":{");
            for (k, (field, problem)) in self.fields.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                field.json(&mut out);
                out.push(':');
                problem.json(&mut out);
            }
            out.push('}');
        }
        out.push('}');
        out
    }

    /// A redirect with a status other than [`redirect`]'s 303, such as 308
    /// for a page that moved for good: `return Err(Error::redirect(308, "/new"))`.
    /// Panics on CR/LF in `location`.
    pub fn redirect(status: u16, location: impl Into<String>) -> Error {
        assert!(
            (300..=308).contains(&status),
            "redirect status must be 3xx, got {status}"
        );
        // A path of the app's own is under its base path, when it has one.
        let location = location.into();
        let location = match crate::protocol::BASE.is_empty() {
            true => location,
            false => crate::protocol::based(&location).into_owned(),
        };
        Error::raw(status, Cow::Borrowed("")).with_header("location", location)
    }

    /// The HTTP status code.
    pub fn status(&self) -> u16 {
        self.status
    }

    /// The message shown to the visitor.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The message and, when it came from another error, that error: for
    /// logs, never for pages.
    pub(crate) fn detail(&self) -> String {
        match &self.source {
            Some(s) => format!("{} ({s:?})", self.message),
            None => self.message.to_string(),
        }
    }
}

/// `return error(404, "No such post")`: stops the `load`, action or
/// endpoint, and the nearest `+error.wisp` shows `message`.
/// [`Error::new`] is the error itself, for `map_err` and the like.
pub fn error<T>(status: u16, message: impl Into<Cow<'static, str>>) -> Result<T> {
    Err(Error::new(status, message))
}

/// `return invalid("email", "is already taken")`: a 422 for input that does
/// not pass, by field, as `#[validate]` gives one. An API client gets
/// `{"errors":{"email":"is already taken"}}`; [`Error::and`] adds fields.
pub fn invalid<T>(field: impl Into<String>, problem: impl Into<String>) -> Result<T> {
    Err(Error::invalid(field, problem))
}

/// `return redirect("/login")`: 303 See Other, which sends the browser
/// to `location` with a GET, whether it came with a form post or a link.
/// [`Error::redirect`] takes other statuses. Panics on CR/LF in `location`.
pub fn redirect<T>(location: impl Into<String>) -> Result<T> {
    Err(Error::redirect(303, location))
}

impl<E: std::error::Error + Send + Sync + 'static> From<E> for Error {
    fn from(e: E) -> Error {
        let message = Cow::Owned(e.to_string());
        Error {
            source: Some(Box::new(e)),
            ..Error::raw(500, message)
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} {}", self.status, self.message)?;
        if let Some((name, value)) = self.header.as_deref() {
            write!(f, " ({name}: {value})")?;
        }
        if let Some(s) = &self.source {
            write!(f, " ({s:?})")?;
        }
        Ok(())
    }
}

/// Turns `Option`/`Result` into HTTP errors: `db.find(id).await?.or_404()?`.
pub trait OrStatus<T> {
    /// `Err` with `status` (and a default message) when `self` is `None` or `Err`: `x.or_status(409)?`.
    fn or_status(self, status: u16) -> Result<T>;

    /// `or_status(404)`.
    fn or_404(self) -> Result<T>
    where
        Self: Sized,
    {
        self.or_status(404)
    }
}

impl<T> OrStatus<T> for Option<T> {
    fn or_status(self, status: u16) -> Result<T> {
        self.ok_or_else(|| Error::new(status, http::reason(status)))
    }
}

impl<T, E: fmt::Display> OrStatus<T> for std::result::Result<T, E> {
    fn or_status(self, status: u16) -> Result<T> {
        self.map_err(|e| Error::new(status, e.to_string()))
    }
}

/// Support for generated code. Not a stable API.
#[doc(hidden)]
pub mod rt {
    pub use crate::envconf::{config, config_error, config_opt};
    pub use crate::i18n::{Arg, Case, Count, Msg, Part, Tr};
    pub use crate::rules::{headers, redirect, rewrite};
    pub use crate::tail::{
        AnyResult, Awaits, Settled, Value, WispResult, defer, failed, failed_html as await_failed,
    };
    pub use crate::timeout::within;

    /// The request's locale, by index: for `Out::lang`.
    #[inline]
    pub fn pick_locale(cx: &crate::Cx) -> u8 {
        crate::i18n::pick(cx)
    }
    pub use crate::bake::{Baked, CacheMore, baked, cached, keep};
    pub use crate::cx::{
        BadCookie, CookieReader, CookieWriter, MAX_PARAMS, MAX_SEGS, decode, split,
    };
    pub use crate::dev::{chunk, marks};
    /// A route parameter in the path a typed route (`routes::post(id)`)
    /// builds: percent-encoded, a rest parameter keeping its `/`s.
    pub fn path_param(out: &mut String, v: &dyn std::fmt::Display, rest: bool) {
        let keep: fn(u8) -> bool = match rest {
            true => |b| crate::cx::unreserved(b) || b == b'/',
            false => crate::cx::unreserved,
        };
        let _ = crate::cx::encode(out, &v.to_string(), keep);
    }
    /// A `#[derive(Rest)]` type's handlers and hooks (see `rest.rs`).
    pub mod rest {
        pub use crate::rest::{Hooks, Kind, create, delete, get, list, patch, put};
    }

    /// A `created_at` or `updated_at` field of a `#[derive(Rest)]` type:
    /// whole seconds since 1970 as a number, or RFC 3339 UTC text
    /// (`2026-09-29T12:00:00Z`) as a `String`.
    pub trait Stamp {
        fn now() -> Self;
    }

    use crate::unix_now;

    macro_rules! stamps {
        ($($t:ty)*) => {$(
            impl Stamp for $t {
                fn now() -> $t {
                    unix_now() as $t
                }
            }
        )*};
    }
    stamps!(u64 i64 u128 i128 f64);

    impl Stamp for String {
        fn now() -> String {
            rfc3339(unix_now())
        }
    }

    /// Seconds since 1970 as `2026-09-29T12:00:00Z`.
    pub(crate) fn rfc3339(s: u64) -> String {
        let ((year, month, day), rem) = (crate::civil(s / 86_400), s % 86_400);
        format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
            rem / 3600,
            rem / 60 % 60,
            rem % 60
        )
    }

    impl<T: Stamp> Stamp for Option<T> {
        fn now() -> Option<T> {
            Some(T::now())
        }
    }

    /// An auto field of an object being read: what it holds, or now when it
    /// is left out.
    pub fn stamped<T: crate::FromJson + Stamp>(
        p: &mut crate::json::Problems,
        members: &[(String, crate::Value)],
        name: &str,
    ) -> Option<T> {
        match members.iter().rev().find(|(k, _)| k == name) {
            Some((_, v)) => p.read(name, v),
            None => Some(T::now()),
        }
    }
    /// A component in the workshop at `/_wisp/components` (dev builds).
    pub struct Shelf {
        pub name: &'static str,
        pub file: &'static str,
        pub props: &'static [ShelfProp],
        /// From its `Name.stories.wisp`, or the default story.
        pub stories: &'static [Story],
        /// Why it has no story, when it has none.
        pub note: &'static str,
    }

    pub struct ShelfProp {
        pub name: &'static str,
        pub ty: &'static str,
        /// How the workshop edits it, if it can.
        pub control: Option<Control>,
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum Control {
        Text,
        Number,
        Check,
    }

    pub struct Story {
        pub name: &'static str,
        pub slug: &'static str,
        /// Where it is: its stories file, or for the default story the
        /// component's.
        pub file: &'static str,
        pub line: u32,
        /// The props' first values, where the story writes literals.
        pub values: &'static [(&'static str, &'static str)],
        /// Renders it, its simple props from the query.
        pub render: fn(&mut crate::Out, &crate::Cx),
    }

    /// The app's service worker and web app manifest ([`crate::App::PWA`]),
    /// as the build made them.
    pub struct Pwa {
        /// `/service-worker.js`, or "".
        pub worker: &'static str,
        pub worker_etag: &'static str,
        /// `/manifest.webmanifest`: `None` when `init` gives it
        /// ([`crate::app_manifest`]), "" without one.
        pub manifest: Option<&'static str>,
        pub manifest_etag: &'static str,
        /// `static/`'s icons, which a manifest from `init` gets.
        pub icons: &'static str,
        /// What every page's head gets: the manifest's link, the script
        /// that registers the worker.
        pub head: &'static str,
    }

    /// What the build knows of a route: one row per route id in
    /// [`crate::App::ROUTES`], so no fact can drift from the others.
    pub struct RouteFacts {
        /// Its parameters' names.
        pub params: &'static [&'static str],
        /// Its own `BODY_LIMIT`, if its `+page.rs` or `+server.rs` sets one.
        pub body_limit: Option<usize>,
        /// The bytes its page's actions take as uploads in all (their
        /// `max_size`), if they take any.
        pub uploads: Option<usize>,
        /// A request to it is answered without waiting on anything: its
        /// `before` hooks, loads, actions, handlers and error page call no
        /// `async fn` of the app's, and its page's statements await
        /// nothing. The server then answers it as it receives it, without
        /// a task's round trip.
        pub now: bool,
        /// The methods ([`crate::Method::bit`]) whose arm
        /// [`crate::App::handle_now`] answers, with no future at all.
        pub sync: u8,
        /// A file the binary embeds may be at one of its paths, and is
        /// served instead: else a GET to it skips looking.
        pub files: bool,
        /// Its nearest `+error.wisp`, by the app's own numbering.
        pub error: Option<usize>,
        /// It has a page, whose address [`crate::trailing_slash`] decides.
        pub page: bool,
        /// Its path as the route folder spells it, `/blog/[slug]`: what
        /// logs, metrics and traces name it by.
        pub pattern: &'static str,
    }

    impl RouteFacts {
        /// A route with parameters `params` and no other facts.
        pub const fn new(params: &'static [&'static str]) -> RouteFacts {
            RouteFacts {
                params,
                body_limit: None,
                uploads: None,
                now: false,
                sync: 0,
                files: true,
                error: None,
                page: false,
                pattern: "",
            }
        }

        /// Its body limit, if not the usual one: with uploads, room for
        /// them on top of the usual limit, or its own `BODY_LIMIT` when
        /// that is more. `usual`: `WISP_BODY_LIMIT`, which the caller has.
        #[inline]
        pub(crate) fn limit(&self, usual: usize) -> Option<usize> {
            let Some(uploads) = self.uploads else {
                return self.body_limit;
            };
            Some((self.body_limit.unwrap_or(usual)).max(usual.saturating_add(uploads)))
        }
    }

    /// `#[remote]` functions' arguments and answers (see `remote.rs`).
    pub mod remote {
        pub use crate::remote::{args, get, members};
    }

    /// A handler's parameters, read by name (see `input.rs`).
    pub mod input {
        pub use crate::input::{all, body, failed, flag, optional, read, refused, required, whole};
    }
    pub use crate::contexts::{escape, guard_url};
    pub use crate::html::{Always, Attr, Direct, Formatted, Maybe, Text, raw as html, text};
    pub use crate::json::to_json as js_of;
    pub use crate::live::{
        Js, RUNTIME_VERSION, js_attr, js_attrs, js_text, json, live, live_end, live_how,
        live_route, same_version, tag_name,
    };
    use crate::{Cx, Error, Out, Response};

    /// The segment at the start of `r`, a path after one of its `/`s, and
    /// the path after the `/` that ends it, if one does: how the router
    /// walks a path.
    #[inline(always)]
    pub fn seg(r: &str) -> (&str, Option<&str>) {
        match r.bytes().position(|b| b == b'/') {
            Some(i) => (&r[..i], Some(&r[i + 1..])),
            None => (r, None),
        }
    }

    /// The text of a `[...rest]` match: from its first segment to the end of
    /// the path, still percent-encoded. Segments are slices of `path`.
    pub fn rest<'a>(path: &'a str, segs: &[&'a str]) -> &'a str {
        match segs.first() {
            None => "",
            Some(first) => {
                let start = first.as_ptr() as usize - path.as_ptr() as usize;
                debug_assert!(start <= path.len());
                &path[start..]
            }
        }
    }

    /// What the form sent for `name`, when an action refused it: an
    /// `<input name="x">` in a `<form action="?/…">` shows it again.
    pub fn kept<'a>(
        cx: &'a Cx,
        refused: Option<&Error>,
        name: &str,
    ) -> Option<std::borrow::Cow<'a, str>> {
        refused?;
        cx.input(name)
    }

    /// What an action refused, if it did: read once a render, for its
    /// form's fields (`kept`, `problem`).
    pub fn refused(cx: &Cx) -> Option<&Error> {
        cx.get::<Error>()
    }

    /// What a `<select>` chooses its option by (see `chosen`): text from
    /// the form, or its own value written into a small buffer (a `String`
    /// only for one too long for it).
    pub enum Chosen<'a> {
        None,
        Kept(std::borrow::Cow<'a, str>),
        Small(u8, [u8; 48]),
    }

    /// The value a `<select>` of an action's form chooses its option by:
    /// what was sent (`kept`), else its own `value={expr}`, as text.
    pub fn chosen<'a, T: std::fmt::Display + ?Sized>(
        own: Option<&T>,
        kept: Option<std::borrow::Cow<'a, str>>,
    ) -> Chosen<'a> {
        use std::fmt::Write;
        if let Some(k) = kept {
            return Chosen::Kept(k);
        }
        let Some(v) = own else { return Chosen::None };
        struct Buf(usize, [u8; 48]);
        impl Write for Buf {
            fn write_str(&mut self, s: &str) -> std::fmt::Result {
                let end = self.0 + s.len();
                self.1
                    .get_mut(self.0..end)
                    .ok_or(std::fmt::Error)?
                    .copy_from_slice(s.as_bytes());
                self.0 = end;
                Ok(())
            }
        }
        let mut b = Buf(0, [0; 48]);
        match write!(b, "{v}") {
            Ok(()) => Chosen::Small(b.0 as u8, b.1),
            Err(_) => Chosen::Kept(std::borrow::Cow::Owned(v.to_string())),
        }
    }

    /// Whether an `<option>`'s value is `chosen`: `selected` then.
    pub fn is<T: std::fmt::Display + ?Sized>(chosen: &Chosen<'_>, value: &T) -> bool {
        // Compared as it is written, without a copy of it.
        struct Rest<'a>(&'a [u8]);
        impl std::fmt::Write for Rest<'_> {
            fn write_str(&mut self, s: &str) -> std::fmt::Result {
                self.0 = self.0.strip_prefix(s.as_bytes()).ok_or(std::fmt::Error)?;
                Ok(())
            }
        }
        let mut rest = Rest(match chosen {
            Chosen::None => return false,
            Chosen::Kept(c) => c.as_bytes(),
            Chosen::Small(n, b) => &b[..*n as usize],
        });
        std::fmt::write(&mut rest, format_args!("{value}")).is_ok() && rest.0.is_empty()
    }

    /// What was wrong with `name`, after its `<input>` in an action's form:
    /// `<small class="problem">…</small>`, or nothing.
    pub fn problem(out: &mut String, refused: Option<&Error>, name: &str) {
        let Some((_, p)) = refused.and_then(|e| e.fields().iter().find(|(f, _)| f == name)) else {
            return;
        };
        out.push_str("<small class=\"problem\">");
        escape(out, p);
        out.push_str("</small>");
    }

    pub fn respond(out: &mut Out, r: Response) {
        out.response = Some(r);
    }

    /// The answer of a handler that returns nothing: a 204.
    pub fn no_content(out: &mut Out) {
        out.made = Some(crate::bake::Made::NoContent);
    }

    /// A POST the hooks let through, with an `Idempotency-Key`: true when
    /// its answer is decided already, the first one again or a refusal, in
    /// `out`; else it is answered as usual, and that answer kept.
    pub fn idempotent(cx: &mut Cx, out: &mut Out) -> bool {
        match crate::idem::start(cx) {
            crate::idem::Start::Skip => false,
            crate::idem::Start::Fresh(key) => {
                cx.idem = Some(key);
                false
            }
            crate::idem::Start::Answered(r) => {
                respond(out, r);
                true
            }
        }
    }

    /// The route has a guard (a rate limit, `CORS`, a middleware), which runs
    /// for every request: the edge build does not keep this answer.
    #[cfg(target_arch = "wasm32")]
    pub fn guarded(cx: &Cx) {
        crate::edge::guarded(cx);
    }

    /// A 500 when the GET of a page is live.js asking for its error page
    /// (`x-wisp-error`): its browser code failed while starting.
    pub fn browser_ok(cx: &Cx) -> crate::Result<()> {
        if cx.method == crate::Method::Get && cx.known(crate::cx::Known::WispError).is_some() {
            return Err(Error::new(500, "Something went wrong in the browser"));
        }
        Ok(())
    }

    /// The request is routed to a `+server.rs` endpoint: its errors are JSON.
    pub fn endpoint(cx: &mut Cx) {
        cx.set_api();
    }

    /// The `before` hook has run: the headers it set stay on this request's
    /// response, error pages included.
    pub fn hooked(cx: &mut Cx) {
        cx.keep_headers();
    }

    /// Form posts must come from our own origin (CSRF). Browsers always send
    /// `Origin` on POST; non-browser clients without it are allowed. The
    /// origin must be `ORIGIN` when that is set, or name the host the request
    /// was sent to: `Host`, or `X-Forwarded-Host` from a proxy (a page on
    /// another site cannot set that header without the app allowing it).
    /// Without `Origin`, a `Sec-Fetch-Site` that names another site (or a
    /// sibling one, `same-site`) is refused too: an older browser, or a
    /// privacy setting, may leave `Origin` out.
    pub fn check_origin(cx: &Cx) -> crate::Result<()> {
        let Some(origin) = cx.known(crate::cx::Known::Origin) else {
            let site = cx.known(crate::cx::Known::SecFetchSite).unwrap_or("none");
            if site.eq_ignore_ascii_case("same-origin") || site.eq_ignore_ascii_case("none") {
                return Ok(());
            }
            return Err(Error::new(403, "Cross-site form submissions are forbidden"));
        };
        if let Some(own) = &crate::settings().origin {
            if origin.eq_ignore_ascii_case(own) {
                return Ok(());
            }
        } else {
            let origin_host = origin.split_once("://").map_or(origin, |(_, h)| h);
            if [cx.header("host"), cx.forwarded("x-forwarded-host")]
                .into_iter()
                .flatten()
                .any(|h| origin_host.eq_ignore_ascii_case(h))
            {
                return Ok(());
            }
        }
        // A proxy that rewrites `Host` refuses every form this way, so the
        // first refusal says how to fix that.
        static TOLD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !TOLD.swap(true, std::sync::atomic::Ordering::Relaxed) {
            crate::http::log(format_args!(
                "wisp: refused a form post from {origin} to host {}\n  If every form is refused, the site is behind a proxy that changes Host: set ORIGIN to the site's own address, such as https://example.com.",
                cx.header("host").unwrap_or("(none)")
            ));
        }
        Err(Error::new(403, "Cross-site form submissions are forbidden"))
    }

    pub fn no_action(name: &str) -> Error {
        Error::new(404, format!("No action named `{name}` on this page"))
    }

    /// The answer to OPTIONS: no body, and what the route takes.
    pub fn options(allow: &'static str) -> Response {
        Response::empty(204).with_header("allow", allow)
    }

    pub fn method_not_allowed(allow: &'static str) -> Error {
        Error::new(405, "Method Not Allowed").with_header("allow", allow)
    }

    /// Used when no `+error.wisp` applies, or when rendering one failed: the
    /// status and one line, centered, on Wisp's dark tokens, with its own
    /// few styles (the app's CSS may not exist yet). The line is the
    /// status's name unless the error says something more specific. Under
    /// `wisp dev` it is [`dev_error`]'s page instead.
    pub fn default_error(cx: &Cx, out: &mut Out, status: u16, message: &str) {
        if crate::settings().dev {
            return dev_error(cx, out, status, message);
        }
        let title = crate::http::title(status);
        let line = if message.is_empty() || message == crate::http::sentence(status) {
            title
        } else {
            message
        };
        out.head.push_str("<title>");
        text(&mut out.head, title);
        out.head.push_str("</title><style>");
        out.head.push_str(crate::http::TOKENS_CSS);
        out.head.push_str(crate::http::ERROR_CSS);
        out.head.push_str("</style>");
        out.body.push_str("<main class=\"wisp-error\"><h1>");
        text(&mut out.body, &status);
        out.body.push_str("</h1><p>");
        text(&mut out.body, line);
        out.body.push_str("</p></main>");
    }

    /// The page of `wisp dev`: the status and its name, the message (in dev
    /// a 5xx's own, with what caused it), the request, and a way home. It
    /// brings Wisp's UI styles; release builds never reach it.
    fn dev_error(cx: &Cx, out: &mut Out, status: u16, message: &str) {
        let title = crate::http::title(status);
        out.head.push_str("<title>");
        text(&mut out.head, title);
        out.head.push_str("</title><style>");
        out.head.push_str(crate::http::UI_CSS);
        out.head.push_str(crate::http::ERROR_CSS);
        out.head.push_str("</style>");
        out.body.push_str("<main class=\"wisp-error\"><h1>");
        text(&mut out.body, &status);
        out.body.push(' ');
        text(&mut out.body, title);
        out.body.push_str("</h1><p>");
        text(&mut out.body, message);
        out.body.push_str("</p><p class=\"wisp-ref\">");
        text(&mut out.body, cx.method.as_str());
        out.body.push(' ');
        text(&mut out.body, cx.path());
        if !cx.query_string().is_empty() {
            out.body.push('?');
            text(&mut out.body, cx.query_string());
        }
        out.body.push_str(
            "</p><div class=\"wisp-actions\"><a class=\"wisp-button wisp-primary\" href=\"/\">Go to the Home Page</a></div></main>",
        );
        // What `wisp-dev.js` opens in its dialog, with `src/..:line` as editor links.
        if status >= 500 {
            out.body.push_str("<template id=\"wisp-server-error\">");
            text(&mut out.body, message);
            out.body.push_str("</template>");
        }
    }
}

#[cfg(test)]
mod tests {
    /// `.env` fills in what the process's environment lacks, never more.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn env_file_fills_in() {
        let file = [("WISP_TEST_ONLY_IN_FILE", "file"), ("PATH", "file")]
            .map(|(k, v)| (k.to_string(), v.to_string()));
        assert_eq!(
            super::var("WISP_TEST_ONLY_IN_FILE", &file).as_deref(),
            Some("file")
        );
        assert!(super::var("PATH", &file).is_some_and(|p| p != "file"));
        assert_eq!(super::var("WISP_TEST_NOWHERE", &file), None);
    }

    /// Every number to 100 000, each power of ten and its neighbours, and
    /// the ends of the range, against `Display`, written where asked.
    #[test]
    fn decimals_two_digits_at_a_time() {
        let mut n = 1u64;
        let mut edges = vec![
            0,
            u64::MAX,
            u64::MAX - 1,
            i64::MAX as u64,
            i64::MIN.unsigned_abs(),
        ];
        while let Some(m) = n.checked_mul(10) {
            edges.extend([n - 1, n, n + 1]);
            n = m;
        }
        edges.extend([n - 1, n, n + 1]);
        for n in (0..=100_000).chain(edges) {
            let mut buf = [b'x'; 24];
            let start = super::digits(&mut buf, 22, n);
            assert_eq!(&buf[start..22], n.to_string().as_bytes());
            assert!(buf[..start].iter().chain(&buf[22..]).all(|&b| b == b'x'));
            let mut s = String::from("=");
            super::decimal(&mut s, n);
            assert_eq!(s, format!("={n}"));
        }
    }

    #[test]
    fn events_keep_every_line_in_data() {
        use super::event_text;
        assert_eq!(event_text("hi"), "data: hi\n\n");
        assert_eq!(event_text(""), "data: \n\n");
        assert_eq!(
            event_text("a\r\nb\nc\rd\n"),
            "data: a\ndata: b\ndata: c\ndata: d\ndata: \n\n"
        );
        // A lone CR ends a line for the browser: what follows it is data
        // still, not a field of its own.
        let sneaky = event_text("x\revent: admin\rid: 9\r\rdata: y");
        assert!(
            sneaky
                .lines()
                .all(|l| l.is_empty() || l.starts_with("data: "))
        );
        assert!(!sneaky.contains('\r'));
    }

    /// A task of `every` that panics runs again the next period.
    #[test]
    fn every_outlives_a_panic() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static RUNS: AtomicU32 = AtomicU32::new(0);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            super::every(std::time::Duration::from_millis(5), || async {
                if RUNS.fetch_add(1, Ordering::Relaxed) == 0 {
                    panic!("the first run fails, on purpose");
                }
            });
            for _ in 0..400 {
                if RUNS.load(Ordering::Relaxed) >= 3 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        });
        assert!(RUNS.load(Ordering::Relaxed) >= 3);
    }

    #[test]
    fn form_posts_from_other_sites_are_refused() {
        let post = |headers: &str| {
            let cx = crate::Cx::for_test(
                &format!("POST /p HTTP/1.1\r\nhost: a.com\r\n{headers}\r\n"),
                &[],
            );
            super::rt::check_origin(&cx).map_err(|e| e.status())
        };
        assert_eq!(post(""), Ok(()), "curl sends no Origin");
        assert_eq!(post("origin: https://a.com\r\n"), Ok(()));
        assert_eq!(post("origin: https://b.com\r\n"), Err(403));
        assert_eq!(post("origin: null\r\n"), Err(403));
        assert_eq!(post("origin: https://a.com.b.com\r\n"), Err(403));
        // No `Origin`: what the browser says of the page that sent it.
        assert_eq!(post("sec-fetch-site: same-origin\r\n"), Ok(()));
        assert_eq!(
            post("sec-fetch-site: none\r\n"),
            Ok(()),
            "typed or bookmarked"
        );
        assert_eq!(post("sec-fetch-site: cross-site\r\n"), Err(403));
        assert_eq!(post("sec-fetch-site: same-site\r\n"), Err(403));
        assert_eq!(
            post("origin: https://a.com\r\nsec-fetch-site: same-origin\r\n"),
            Ok(())
        );
    }

    #[test]
    fn provided_values_reach_every_thread() {
        struct Answer(u32);
        super::provide(Answer(1));
        assert_eq!(super::state::<Answer>().0, 1);
        super::provide(Answer(2));
        assert_eq!(super::state::<Answer>().0, 2, "a copy made before is stale");
        let other = std::thread::spawn(|| super::state::<Answer>().0);
        assert_eq!(other.join().unwrap(), 2);
    }

    #[test]
    fn timestamps_and_error_codes() {
        use super::rt::rfc3339;
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_868_800), "2000-03-01T00:00:00Z");
        assert_eq!(rfc3339(1_709_164_800), "2024-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_735_689_599), "2024-12-31T23:59:59Z");

        let e = super::Error::invalid("title", "is required");
        assert_eq!(
            e.json("title: is required", false),
            r#"{"status":422,"code":"invalid","error":"title: is required","errors":{"title":"is required"}}"#
        );
        let e = super::Error::new(409, "Taken").with_code("email_taken");
        assert_eq!(
            e.json("Taken", true),
            r#"{"type":"about:blank","title":"Conflict","status":409,"code":"email_taken","detail":"Taken"}"#
        );
        assert_eq!(super::Error::new(418, "x").code(), "client_error");
    }

    #[test]
    fn a_select_chooses_by_text() {
        use super::rt::{chosen, is};
        use std::borrow::Cow;
        let kept = chosen(Some(&3u8), Some(Cow::Borrowed("7")));
        assert!(is(&kept, &7) && !is(&kept, &3), "what was sent first");
        let own = chosen(Some(&7u64), None);
        assert!(is(&own, &7) && is(&own, "7"));
        assert!(!is(&own, &70) && !is(&own, "") && !is(&own, &"77"));
        let long = chosen(Some(&"x".repeat(60)), None);
        assert!(is(&long, &"x".repeat(60)) && !is(&long, &"x".repeat(59)));
        assert!(!is(&chosen(None::<&str>, None), ""), "nothing is chosen");
    }
}
