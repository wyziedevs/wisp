//! Wisp: a fast, fun web framework for Rust. File-based routes, `.wisp`
//! templates, form actions, one binary. See <https://wispweb.dev/docs/design> for the whole picture.
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

mod bake;
mod cache;
#[cfg(not(target_arch = "wasm32"))]
mod channel;
mod codec;
mod compress;
mod content;
mod csp;
mod cx;
mod dev;
#[cfg(target_os = "linux")]
mod driver;
#[cfg(target_arch = "wasm32")]
pub mod edge;
#[cfg(target_arch = "wasm32")]
mod edge_store;
mod env;
mod envconf;
#[cfg(target_os = "linux")]
mod epoll;
mod error;
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
mod image;
#[cfg(feature = "img")]
pub mod img;
mod input;
mod intl;
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
mod remote;
mod response;
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
mod ws;

pub use cache::{cache, revalidate_tag, uncache};
#[cfg(not(target_arch = "wasm32"))]
pub use channel::{Channel, Subscription, channel};
pub use content::{MdPage, pages};
pub use csp::{csp, csp_off};
pub use cx::{CookieOptions, Cx, Method, SameSite};
#[cfg(target_arch = "wasm32")]
pub use edge::fetch;
pub(crate) use env::*;
pub use env::{address, env, env_or};
pub use error::{Error, OrStatus, error, invalid, redirect};
pub use export::{Entry, ExportRoute, export, prerender};
#[cfg(not(target_arch = "wasm32"))]
pub use fetch::{fetch, on_fetch};
pub use form::{File, Form};
pub use http::{Body, Reply, Request, TrailingSlash, handle, trailing_slash};
pub use i18n::{
    Prefix, alternates, default_locale, dir, locales, localize, native_name, prefix, switcher,
};
pub use image::Image;
pub use input::Email;
pub use intl::{AsDate, format_date, format_date_long, format_money, format_number};
pub use jobs::{Queue, cron, queue, work};
pub use json::{FromJson, Value, from_json, to_json};
pub use limit::RateLimit;
pub use live::{ClientModule, Json};
pub use otel::{SpanGuard, span, traceparent};
pub use password::Password;
pub use pwa::app_manifest;
pub use response::{Gone, IntoText, Response, Sender};
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

/// `Some("page")` when `path` (the request's, `cx.path()`) is `href` or
/// below it, else `None`; `/` matches only itself, a trailing `/` and a
/// query on either side are ignored. `<a href="/blog" active>` writes
/// `aria-current={wisp::current(cx.path(), "/blog")}` for you:
/// `assert_eq!(wisp::current("/blog/x", "/blog"), Some("page"))`.
pub fn current(path: &str, href: &str) -> Option<&'static str> {
    fn trim(p: &str) -> &str {
        let t = p
            .split(['?', '#'])
            .next()
            .unwrap_or("")
            .trim_end_matches('/');
        if t.is_empty() { "/" } else { t }
    }
    let (path, href) = (trim(path), trim(href));
    let hit = path == href
        || href != "/" && path.starts_with(href) && path.as_bytes().get(href.len()) == Some(&b'/');
    hit.then_some("page")
}

#[cfg(test)]
mod current_tests {
    #[test]
    fn current_is_the_page_or_below() {
        use super::current;
        assert_eq!(current("/blog", "/blog"), Some("page"));
        assert_eq!(current("/blog/x/", "/blog"), Some("page"));
        assert_eq!(current("/blog?p=2", "/blog/"), Some("page"));
        assert_eq!(current("/blogs", "/blog"), None);
        assert_eq!(current("/about", "/"), None);
        assert_eq!(current("/", "/"), Some("page"));
        assert_eq!(current("/", "/#top"), Some("page"));
    }
}
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
        OrStatus, Password, Reply, Response, Rest, Result, Row, SameSite, Shared, Table, Value,
        action, error, invalid, model, redirect, remote,
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
        // A route folder `[userId]` is a parameter of that name in the
        // generated code, which Rust would warn about on a file it never shows.
        #[doc(hidden)]
        #[allow(non_snake_case)]
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

/// [`prepare`] with no app code and nothing a worker's global scope does not
/// allow: no `init`, no sessions (whose secret may be random). For an edge
/// warm-up instance (`edge::warming`).
#[cfg(target_arch = "wasm32")]
pub(crate) fn prepare_warm<A: App>() {
    settings();
    http::setup::<A>();
    csp::ready(
        A::SCRIPT_HASHES,
        A::PWA.is_some_and(|p| !p.worker.is_empty()),
    );
    content::ready(A::PAGES);
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
    #[cfg(not(target_arch = "wasm32"))]
    out.push_str(std::str::from_utf8(&buf[start..]).unwrap_or_default());
    // In wasm `from_utf8` is an outlined call that checks bytes known to be
    // ASCII digits: pushing them one by one is shorter.
    #[cfg(target_arch = "wasm32")]
    {
        out.reserve(20 - start);
        for &d in &buf[start..] {
            out.push(char::from(d));
        }
    }
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
    /// What goes in `<html lang="…">` per locale: [`App::LOCALES`], and a
    /// right-to-left one with its `dir` (`ar" dir="rtl`).
    const HTML_LANGS: &'static [&'static str] = Self::LOCALES;
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

/// Support for generated code. Not a stable API.
#[doc(hidden)]
pub mod rt {
    pub use crate::envconf::{config, config_error, config_opt};
    pub use crate::i18n::{Arg, Case, Count, Msg, Part, Tr};

    /// At startup, from `i18n = [...]`: the default locale, the prefix and
    /// the domains with their locales' indexes.
    pub fn locale_setup(default: u8, prefix: u8, domains: &'static [(&'static str, u8)]) {
        crate::i18n::setup(default, prefix, domains);
    }

    /// The redirect of a page under `[[lang=locale]]` that `i18n`'s prefix
    /// asks for, as its error.
    pub fn locale_redirect(cx: &crate::Cx) -> crate::Result {
        crate::i18n::redirect(cx)
    }
    pub use crate::rules::{headers, redirect, rewrite};
    pub use crate::tail::{
        AnyResult, Awaits, Settled, Value, WispResult, defer, failed, failed_html as await_failed,
    };
    pub use crate::timeout::within;

    /// What `{#if x}` tests when `x` is a bare place: a `bool` itself, an
    /// `Option` when it is `Some`, a string, list, set or map when it is not empty.
    #[diagnostic::on_unimplemented(
        message = "`{{#if}}` cannot test a `{Self}`: write a bool expression",
        label = "not a bool, Option, string or list"
    )]
    pub trait Truthy {
        /// Whether `{#if}` takes its branch.
        fn truthy(&self) -> bool;
    }
    impl Truthy for bool {
        #[inline(always)]
        fn truthy(&self) -> bool {
            *self
        }
    }
    impl<T> Truthy for Option<T> {
        #[inline(always)]
        fn truthy(&self) -> bool {
            self.is_some()
        }
    }
    impl Truthy for str {
        #[inline(always)]
        fn truthy(&self) -> bool {
            !self.is_empty()
        }
    }
    impl Truthy for String {
        #[inline(always)]
        fn truthy(&self) -> bool {
            !self.is_empty()
        }
    }
    impl<T> Truthy for [T] {
        #[inline(always)]
        fn truthy(&self) -> bool {
            !self.is_empty()
        }
    }
    impl<T> Truthy for Vec<T> {
        #[inline(always)]
        fn truthy(&self) -> bool {
            !self.is_empty()
        }
    }
    impl Truthy for std::borrow::Cow<'_, str> {
        #[inline(always)]
        fn truthy(&self) -> bool {
            !self.is_empty()
        }
    }
    impl<T> Truthy for std::collections::VecDeque<T> {
        #[inline(always)]
        fn truthy(&self) -> bool {
            !self.is_empty()
        }
    }
    impl<K, V, S> Truthy for std::collections::HashMap<K, V, S> {
        #[inline(always)]
        fn truthy(&self) -> bool {
            !self.is_empty()
        }
    }
    impl<T, S> Truthy for std::collections::HashSet<T, S> {
        #[inline(always)]
        fn truthy(&self) -> bool {
            !self.is_empty()
        }
    }
    impl<K, V> Truthy for std::collections::BTreeMap<K, V> {
        #[inline(always)]
        fn truthy(&self) -> bool {
            !self.is_empty()
        }
    }
    impl<T> Truthy for std::collections::BTreeSet<T> {
        #[inline(always)]
        fn truthy(&self) -> bool {
            !self.is_empty()
        }
    }
    impl<T: Truthy + ?Sized> Truthy for &T {
        #[inline(always)]
        fn truthy(&self) -> bool {
            (**self).truthy()
        }
    }

    #[cfg(test)]
    mod truthy_tests {
        use super::truthy;
        #[test]
        fn what_if_tests() {
            assert!(truthy(&true) && !truthy(&false));
            assert!(truthy(&Some(0)) && !truthy(&None::<u8>));
            assert!(truthy(&"a") && !truthy(&"") && !truthy(&String::new()));
            assert!(truthy(&vec![1]) && !truthy(&Vec::<u8>::new()));
            assert!(!truthy(&std::collections::HashMap::<u8, u8>::new()));
            assert!(truthy(&&Some("x")));
        }
    }

    /// `{#if x}` on a bare place `x`: see [`Truthy`].
    #[inline(always)]
    pub fn truthy<T: Truthy + ?Sized>(v: &T) -> bool {
        v.truthy()
    }

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

    /// Whether a refused form sent `value` for checkbox or radio `name`
    /// (any of its values, for a group of checkboxes): `None` when
    /// `action` did not refuse it, so the field shows its own `checked`.
    pub fn ticked<T: std::fmt::Display + ?Sized>(
        cx: &Cx,
        refused: Option<&Error>,
        action: &str,
        name: &str,
        value: &T,
    ) -> Option<bool> {
        Some(is(&Chosen::Sent(sent(cx, refused, action, name)?), value))
    }

    /// The form `action` refused, if it was that one, with the name of one
    /// of its fields: what a `<select>` (`multiple` too) or checkbox shows
    /// again. Another form on the page shows its own values.
    pub fn sent<'a>(
        cx: &'a Cx,
        refused: Option<&Error>,
        action: &str,
        name: &'a str,
    ) -> Option<(crate::Form<'a>, &'a str)> {
        refused?;
        if cx.action() != action {
            return None;
        }
        Some((cx.form(), name))
    }

    /// What an action refused, if it did: read once a render, for its
    /// form's fields (`kept`, `problem`).
    pub fn refused(cx: &Cx) -> Option<&Error> {
        cx.get::<Error>()
    }

    /// What a `<select>` chooses its options by (see `chosen`): every value
    /// the form sent for it (a `<select multiple>` sends several), or its
    /// own value written into a small buffer (a `String` only for one too
    /// long for it).
    pub enum Chosen<'a> {
        None,
        Sent((crate::Form<'a>, &'a str)),
        Kept(std::borrow::Cow<'a, str>),
        Small(u8, [u8; 48]),
    }

    /// The value a `<select>` of an action's form chooses its option by:
    /// what was sent (`sent`), else its own `value={expr}`, as text.
    pub fn chosen<'a, T: std::fmt::Display + ?Sized>(
        own: Option<&T>,
        sent: Option<(crate::Form<'a>, &'a str)>,
    ) -> Chosen<'a> {
        use std::fmt::Write;
        if let Some(s) = sent {
            return Chosen::Sent(s);
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
            Chosen::Sent((form, name)) => {
                return form.all(name).any(|v| is(&Chosen::Kept(v), value));
            }
            Chosen::Kept(c) => c.as_bytes(),
            Chosen::Small(n, b) => &b[..*n as usize],
        });
        std::fmt::write(&mut rest, format_args!("{value}")).is_ok() && rest.0.is_empty()
    }

    #[cfg(test)]
    #[test]
    fn a_refused_form_keeps_every_choice() {
        let cx = Cx::for_test(
            "POST /p?/save HTTP/1.1\r\nContent-Type: application/x-www-form-urlencoded\r\n\r\ntag=a&tag=c&on=on",
            &[],
        );
        let no = Error::redirect(303, "/");
        let refused = Some(&no);
        let sel = chosen(Some("b"), sent(&cx, refused, "save", "tag"));
        assert!(is(&sel, "a") && !is(&sel, "b") && is(&sel, "c"));
        // Another action's refusal leaves this form's own value.
        let sel = chosen(Some("b"), sent(&cx, refused, "other", "tag"));
        assert!(is(&sel, "b") && !is(&sel, "a"));
        assert_eq!(ticked(&cx, refused, "save", "on", "on"), Some(true));
        assert_eq!(ticked(&cx, refused, "save", "off", "on"), Some(false));
        assert_eq!(ticked(&cx, refused, "save", "tag", &'c'), Some(true));
        assert_eq!(ticked(&cx, None, "save", "on", "on"), None);
        assert_eq!(ticked(&cx, refused, "other", "on", "on"), None);
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

    /// The route has a guard (a rate limit, `CORS`, a middleware), which runs
    /// for every request: the edge build does not keep this answer.
    #[cfg(target_arch = "wasm32")]
    pub fn guarded(cx: &Cx) {
        crate::edge::guarded(cx);
    }

    /// A page that says `{@flash}` (or a layout of it does): takes the
    /// message [`Cx::flash`] left, once, for [`Cx::flash_message`].
    pub fn take_flash(cx: &mut Cx) {
        if let Some(m) = cx.flashed() {
            cx.set(crate::cx::Flashed(m));
        }
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
        #[cfg(debug_assertions)]
        crate::http::mark(cx, 0);
    }

    /// The page's loads are done and it renders: where a dev build's
    /// `Server-Timing` ends `handler` and starts `render`. Nothing at all
    /// in a release build.
    #[inline(always)]
    pub fn rendering(cx: &mut Cx) {
        #[cfg(debug_assertions)]
        crate::http::mark(cx, 1);
        let _ = cx;
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
    /// `wisp dev` it is [`dev_error`]'s page instead (debug builds only:
    /// a release build with `WISP_DEV=on` keeps this page).
    pub fn default_error(cx: &Cx, out: &mut Out, status: u16, message: &str) {
        if cfg!(debug_assertions) && crate::settings().dev {
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
    /// A location with CR/LF is a 500 and no panic; a good one keeps its header.
    #[test]
    fn a_bad_redirect_is_a_500_not_a_panic() {
        let e = super::Error::redirect(303, "/x\r\nset-cookie: a=b");
        assert_eq!((e.status(), e.header.is_none()), (500, true));
        let e = super::Error::redirect(308, "/new");
        assert_eq!(e.status(), 308);
        assert!(e.header.is_some());
        let e = super::Error::redirect(200, "/new");
        assert_eq!((e.status(), e.header.is_some()), (303, true));
    }

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
        use crate::response::event_text;
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
        let own = chosen(Some(&7u64), None);
        assert!(is(&own, &7) && is(&own, "7"));
        assert!(!is(&own, &70) && !is(&own, "") && !is(&own, &"77"));
        let long = chosen(Some(&"x".repeat(60)), None);
        assert!(is(&long, &"x".repeat(60)) && !is(&long, &"x".repeat(59)));
        assert!(!is(&chosen(None::<&str>, None), ""), "nothing is chosen");
    }
}
