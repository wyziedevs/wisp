//! Wisp: a fast, fun web framework for Rust. File-based routes, `.wisp`
//! templates, form actions, one binary. See `docs/design.md` for the whole picture.
//!
//! An app's `main.rs` is `wisp::main!();`; everything else is generated
//! from `src/routes` by `wisp-build`.

// No `unsafe` in a native build. The edge build's exports and imports are
// the one exception (`edge.rs`).
#![cfg_attr(not(target_arch = "wasm32"), forbid(unsafe_code))]

mod cx;
mod dev;
#[cfg(target_arch = "wasm32")]
pub mod edge;
mod export;
mod form;
mod html;
mod http;
mod input;
mod live;
mod sign;
#[cfg(not(target_arch = "wasm32"))]
pub mod test;
#[cfg(feature = "tower")]
pub mod tower;
mod ws;

pub use cx::{CookieOptions, Cx, Method, SameSite};
pub use export::{Entry, ExportRoute, export};
pub use form::{File, Form};
pub use http::{Body, Reply, Request, handle};
pub use live::{ClientModule, Json};
pub use wisp_macros::{Cookie, Json, action};
pub use ws::{Message, WebSocket};

use std::any::{Any, TypeId};
use std::borrow::Cow;
use std::fmt;
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::str::FromStr;
use std::sync::{OnceLock, RwLock};

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// What every `+page.rs`, `+layout.rs`, `+server.rs` and `src/hooks.rs`
/// sees without a `use` line. Other Rust files can `use wisp::prelude::*`.
pub mod prelude {
    pub use crate::{
        Cookie, CookieOptions, Cx, Error, Json, Method, OrStatus, Response, Result, SameSite,
        action, error, redirect,
    };
}

/// Sizes for `BODY_LIMIT`: `const BODY_LIMIT: usize = 20 * wisp::MB;`
pub const KB: usize = 1024;
pub const MB: usize = 1024 * KB;

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

/// Includes the code `wisp-build` generated and brings `App` into scope,
/// for a `main` of your own: `wisp::app!(); fn main() { setup(); wisp::run::<App>(); }`.
/// `src/hooks.rs` is `crate::hooks`, so routes can use what it defines.
#[macro_export]
macro_rules! app {
    () => {
        #[doc(hidden)]
        mod __wisp {
            include!(concat!(env!("OUT_DIR"), "/wisp.rs"));
        }
        use __wisp::App;
        #[allow(unused_imports)]
        use __wisp::hooks;
    };
}

/// Serves the app on `$HOST:$PORT`. Defaults to port 3000 on 127.0.0.1 in
/// debug builds and 0.0.0.0 in release builds.
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
    // `wisp build --static` runs the app this way, to write its pages out.
    if let Some(dir) = setting::<String>("WISP_EXPORT", "a folder") {
        return export::run::<A>(&dir).unwrap_or_else(|e| fail(&e.to_string()));
    }
    let addr = address();
    let threads = match setting("WISP_THREADS", "a number of threads above 0") {
        Some(0) => fail("WISP_THREADS is 0, which is not a number of threads above 0"),
        Some(n) => n,
        None => std::thread::available_parallelism().map_or(1, |n| n.get()),
    };
    settings();
    if let Err(e) = http::run::<A>(addr, threads) {
        fail(&e.to_string());
    }
}

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
    A::init()
        .await
        .map_err(|e| std::io::Error::other(format!("init in src/hooks.rs failed: {}", e.detail())))
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
}

/// The `T` given to [`provide`]: `wisp::state::<Db>().query(...)`.
///
/// Panics, naming the type, if none was: that is a missing line at startup,
/// found by the first request that needs it.
pub fn state<T: Send + Sync + 'static>() -> &'static T {
    let all = STATE.read().unwrap_or_else(|e| e.into_inner());
    let found = all
        .iter()
        .find(|(t, _)| *t == TypeId::of::<T>())
        .and_then(|(_, v)| v.downcast_ref::<T>());
    match found {
        Some(v) => v,
        None => panic!(
            "no {} was provided: call wisp::provide(...) with one in `init`, in src/hooks.rs",
            std::any::type_name::<T>()
        ),
    }
}

/// A variable from the host's environment, such as an API key: the
/// process's environment on a server, the worker's variables and secrets
/// on an edge host.
pub fn env(key: &str) -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    return std::env::var(key).ok();
    #[cfg(target_arch = "wasm32")]
    return edge::env(key);
}

/// Runs `task` in the background: the body of a [`Response::stream`], work
/// that outlives its request. On the built-in server it is `tokio::spawn`,
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

/// Values given to [`provide`], leaked: they live as long as the process.
static STATE: RwLock<Vec<(TypeId, &'static (dyn Any + Send + Sync))>> = RwLock::new(Vec::new());

/// `$HOST:$PORT`, with the defaults described in [`run`]. `HOST` is an IP
/// address or a name such as `localhost`.
/// Ends the process with a message if either is not valid.
pub fn address() -> SocketAddr {
    let port: u16 = setting("PORT", "a port number from 0 to 65535").unwrap_or(3000);
    let Some(host) = setting::<String>("HOST", "an address") else {
        let ip = if cfg!(debug_assertions) {
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
    #[cfg(not(target_arch = "wasm32"))]
    let value = std::env::var_os(name)?;
    #[cfg(target_arch = "wasm32")]
    let value = std::ffi::OsString::from(edge::env(name)?);
    match value.to_str().map(|v| v.trim().parse()) {
        Some(Ok(v)) => Some(v),
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
    /// `WISP_WS_IDLE`: seconds a WebSocket client may stay quiet (60; 0
    /// never closes). It is pinged halfway.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))] // no upgrades there
    pub ws_idle: std::time::Duration,
    /// `WISP_MAX_CONNS`: open connections, WebSockets too, past which the
    /// built-in server answers new ones 503 and closes them (10000; 0 is
    /// no cap).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))] // no sockets there
    pub max_conns: usize,
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
        let secret = setting::<String>("WISP_SECRET", "a secret");
        if let Some(s) = &secret
            && s.len() < 32
        {
            fail(&format!(
                "WISP_SECRET is {} characters, too short to keep signed cookies safe\n  Use at least 32 random ones: `openssl rand -hex 32` makes some.",
                s.len()
            ));
        }
        let ws_idle = std::time::Duration::from_secs(setting::<u64>("WISP_WS_IDLE", "a number of seconds").unwrap_or(60));
        let max_conns = match setting::<usize>("WISP_MAX_CONNS", "a number of connections") {
            Some(0) => usize::MAX,
            n => n.unwrap_or(10_000),
        };
        Settings { body_limit, origin, client_ip_header, secret, ws_idle, max_conns }
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
    /// Parameter names per route id.
    const PARAMS: &'static [&'static [&'static str]];
    /// `(path, shape)` per template id, for dev hot swapping.
    const TEMPLATES: &'static [(&'static str, u64)];

    fn route<'a>(path: &'a str, segs: &[&'a str]) -> Option<(usize, [&'a str; cx::MAX_PARAMS])>;
    /// The route's own `BODY_LIMIT`, if its `+page.rs` or `+server.rs` sets one.
    fn body_limit(route: usize) -> Option<usize>;
    fn shell() -> [&'static str; 3];
    fn asset(path: &str) -> Option<&'static Asset>;
    /// The browser module of a template with client code, by its path
    /// (`/_app/c/t3.js`).
    fn client_module(path: &str) -> Option<&'static ClientModule> {
        let _ = path;
        None
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
    fn error(
        route: Option<usize>,
        cx: &mut Cx,
        out: &mut Out,
        status: u16,
        message: &str,
    ) -> impl Future<Output = Result<()>> + Send;
}

/// Where a request's output goes: HTML for the head and body of the shell,
/// or a complete response from a `+server.rs` endpoint.
#[derive(Default)]
pub struct Out {
    pub head: String,
    pub body: String,
    response: Option<Response>,
    /// The template instances with browser code the page rendered.
    live: live::Live,
}

impl Out {
    fn clear(&mut self) {
        self.head.clear();
        self.body.clear();
        self.response = None;
        self.live.clear();
    }
}

/// A file embedded in a release binary.
pub struct Asset {
    pub body: &'static [u8],
    pub ext: &'static str,
    pub etag: &'static str,
}

/// A complete response: from a `+server.rs` endpoint, or from an action or
/// the `before` hook, in place of the page.
pub struct Response {
    pub status: u16,
    pub content_type: Cow<'static, str>,
    pub headers: Vec<(Cow<'static, str>, String)>,
    pub body: Vec<u8>,
    /// For [`Response::stream`]: the body, as it is made.
    stream: Option<tokio::sync::mpsc::Receiver<Vec<u8>>>,
    /// For [`Response::websocket`]: what runs once upgraded.
    upgrade: Option<ws::Upgrade>,
}

impl Response {
    pub fn new(content_type: impl Into<Cow<'static, str>>, body: impl Into<Vec<u8>>) -> Response {
        Response {
            status: 200,
            content_type: content_type.into(),
            headers: Vec::new(),
            body: body.into(),
            stream: None,
            upgrade: None,
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

    /// [`Response::stream`] for a body written from elsewhere: the response
    /// and the [`Sender`] that writes it, which ends when it is dropped.
    /// This is what `Response::stream` was before it took a closure.
    pub fn channel(content_type: impl Into<Cow<'static, str>>) -> (Response, Sender) {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let mut res = Response::new(content_type, Vec::new());
        res.stream = Some(rx);
        (res, Sender(tx))
    }

    /// Sends the browser to `location` with a GET (303): the answer of an
    /// endpoint that does not return a `Result`. Panics on CR/LF in it.
    pub fn redirect(location: impl Into<String>) -> Response {
        Response::empty(303).with_header("location", location)
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
    /// holds back. `send` sends each one with [`Sender::event`]:
    ///
    /// ```ignore
    /// Response::events(|events| async move {
    ///     loop {
    ///         events.event(&now()).await?; // stops once the client has left
    ///         wisp::sleep(Duration::from_secs(1)).await;
    ///     }
    /// })
    /// ```
    pub fn events<F, Fut>(send: F) -> Response
    where
        F: FnOnce(Sender) -> Fut,
        Fut: Future<Output = Result<(), Gone>> + Send + 'static,
    {
        let (res, tx) = Response::event_channel();
        let task = send(tx);
        spawn(async move {
            let _ = task.await;
        });
        res
    }

    /// [`Response::events`] with the [`Sender`] handed back, as it was
    /// before `events` took a closure.
    pub fn event_channel() -> (Response, Sender) {
        let (res, tx) = Response::channel("text/event-stream");
        // `x-accel-buffering` stops nginx from holding events back.
        (
            res.with_header("cache-control", "no-store")
                .with_header("x-accel-buffering", "no"),
            tx,
        )
    }

    pub fn text(body: impl Into<String>) -> Response {
        Response::new("text/plain; charset=utf-8", body.into())
    }

    pub fn html(body: impl Into<String>) -> Response {
        Response::new("text/html; charset=utf-8", body.into())
    }

    /// Serialize with whatever you like; this only sets the content type.
    pub fn json(body: impl Into<String>) -> Response {
        Response::new("application/json", body.into())
    }

    /// `value` as JSON, with `#[derive(Json)]` or one of the built-in impls.
    pub fn json_of(value: &(impl Json + ?Sized)) -> Response {
        let mut body = String::new();
        value.json(&mut body);
        Response::json(body)
    }

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
        let mut out = String::with_capacity(data.len() + 8);
        for line in data.split('\n') {
            out.push_str("data: ");
            out.push_str(line.strip_suffix('\r').unwrap_or(line));
            out.push('\n');
        }
        out.push('\n');
        self.send(out).await
    }

    /// Whether the client has gone, so nothing more can reach it.
    pub fn is_closed(&self) -> bool {
        self.0.is_closed()
    }
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
    /// `location` for redirects, `allow` for 405.
    header: Option<(&'static str, String)>,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl Error {
    pub fn new(status: u16, message: impl Into<Cow<'static, str>>) -> Error {
        assert!(
            (400..=599).contains(&status),
            "error status must be 4xx or 5xx, got {status}"
        );
        Error {
            status,
            message: message.into(),
            header: None,
            source: None,
        }
    }

    /// A redirect with a status other than [`redirect`]'s 303, such as 308
    /// for a page that moved for good: `return Err(Error::redirect(308, "/new"))`.
    /// Panics on CR/LF in `location`.
    pub fn redirect(status: u16, location: impl Into<String>) -> Error {
        let location = location.into();
        assert!(
            (300..=308).contains(&status),
            "redirect status must be 3xx, got {status}"
        );
        assert!(
            cx::valid_header("location", &location),
            "invalid redirect location {location:?}"
        );
        Error {
            status,
            message: Cow::Borrowed(""),
            header: Some(("location", location)),
            source: None,
        }
    }

    pub fn status(&self) -> u16 {
        self.status
    }

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

/// `return redirect("/login")`: 303 See Other, which sends the browser
/// to `location` with a GET, whether it came with a form post or a link.
/// [`Error::redirect`] takes other statuses. Panics on CR/LF in `location`.
pub fn redirect<T>(location: impl Into<String>) -> Result<T> {
    Err(Error::redirect(303, location))
}

impl<E: std::error::Error + Send + Sync + 'static> From<E> for Error {
    fn from(e: E) -> Error {
        Error {
            status: 500,
            message: Cow::Owned(e.to_string()),
            header: None,
            source: Some(Box::new(e)),
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{} {}", self.status, self.message)?;
        if let Some((name, value)) = &self.header {
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
    fn or_status(self, status: u16) -> Result<T>;

    fn or_404(self) -> Result<T>
    where
        Self: Sized,
    {
        self.or_status(404)
    }

    fn or_400(self) -> Result<T>
    where
        Self: Sized,
    {
        self.or_status(400)
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
    pub use crate::cx::{BadCookie, CookieReader, CookieWriter, MAX_PARAMS, decode};
    pub use crate::dev::chunk;
    /// A handler's parameters, read by name (see `input.rs`).
    pub mod input {
        pub use crate::input::{all, flag, optional, required};
    }
    pub use crate::html::{
        Always, Attr, Direct, Formatted, Maybe, Text, escape, guard_url, raw as html, text,
    };
    pub use crate::live::{Js, js_of, js_text, json, live, live_end, live_route, same_version};
    use crate::{Cx, Error, Out, Response};

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

    pub fn respond(out: &mut Out, r: Response) {
        out.response = Some(r);
    }

    /// The `before` hook has run: the headers it set stay on this request's
    /// response, error pages included.
    pub fn hooked(cx: &mut Cx) {
        cx.kept_headers = cx.out_headers.len();
    }

    /// Form posts must come from our own origin (CSRF). Browsers always send
    /// `Origin` on POST; non-browser clients without it are allowed. The
    /// origin must be `ORIGIN` when that is set, or name the host the request
    /// was sent to: `Host`, or `X-Forwarded-Host` from a proxy (a page on
    /// another site cannot set that header without the app allowing it).
    pub fn check_origin(cx: &Cx) -> crate::Result<()> {
        let Some(origin) = cx.header("origin") else {
            return Ok(());
        };
        if let Some(own) = &crate::settings().origin {
            if origin.eq_ignore_ascii_case(own) {
                return Ok(());
            }
        } else {
            let origin_host = origin.split_once("://").map_or(origin, |(_, h)| h);
            let forwarded = cx
                .header("x-forwarded-host")
                .and_then(|h| h.split(',').next())
                .map(str::trim);
            if [cx.header("host"), forwarded]
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

    pub fn method_not_allowed(allow: &'static str) -> Error {
        Error {
            status: 405,
            message: "Method Not Allowed".into(),
            header: Some(("allow", allow.into())),
            source: None,
        }
    }

    /// Used when no `+error.wisp` applies, or when rendering one failed. It
    /// says what happened, what it means or what to do, and the status with
    /// the request as the reference line. It brings its own styles
    /// (`client/ui.css`), since the app's own CSS may not exist yet.
    pub fn default_error(cx: &Cx, out: &mut Out, status: u16, message: &str) {
        let title = crate::http::title(status);
        out.head.push_str("<title>");
        text(&mut out.head, title);
        out.head.push_str("</title><style>");
        out.head.push_str(crate::http::UI_CSS);
        out.head.push_str("</style>");

        // A 5xx failed; it wears the failure glyph. A 4xx is an answer about
        // the request and stays gray.
        out.body.push_str("<main class=\"wisp-error\"><h1>");
        if status >= 500 {
            out.body.push_str(FAILED_ICON);
        }
        text(&mut out.body, title);
        out.body.push_str("</h1><p>");
        text(&mut out.body, message);
        out.body.push_str("</p><p class=\"wisp-ref\">");
        text(&mut out.body, &status);
        out.body.push_str(" · ");
        text(&mut out.body, cx.method.as_str());
        out.body.push(' ');
        text(&mut out.body, cx.path());
        if !cx.query_string().is_empty() {
            out.body.push('?');
            text(&mut out.body, cx.query_string());
        }
        out.body.push_str("</p><div class=\"wisp-actions\">");
        // The same address again is worth a try when the server failed at
        // something that may pass; a post is not repeated behind a link.
        if status >= 500 && matches!(cx.method, crate::Method::Get | crate::Method::Head) {
            out.body
                .push_str("<a class=\"wisp-button wisp-primary\" href=\"");
            crate::html::escape(&mut out.body, cx.path());
            if !cx.query_string().is_empty() {
                out.body.push('?');
                crate::html::escape(&mut out.body, cx.query_string());
            }
            out.body.push_str(
                "\">Try Again</a><a class=\"wisp-button\" href=\"/\">Go to the Home Page</a>",
            );
        } else {
            out.body.push_str(
                "<a class=\"wisp-button wisp-primary\" href=\"/\">Go to the Home Page</a>",
            );
        }
        out.body.push_str("</div></main>");
    }

    /// The failure glyph, shared with the build error dialog.
    const FAILED_ICON: &str = "<svg viewBox=\"0 0 16 16\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.5\" aria-hidden=\"true\">\
<circle cx=\"8\" cy=\"8\" r=\"6.25\"/><path d=\"M8 4.75v3.75\" stroke-linecap=\"round\"/>\
<circle cx=\"8\" cy=\"11\" r=\".75\" fill=\"currentColor\" stroke=\"none\"/></svg>";
}
