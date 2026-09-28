//! Wisp: a fast, fun web framework for Rust. File-based routes, `.wisp`
//! templates, form actions, one binary. See `docs/design.md` for the whole picture.
//!
//! An app's `main.rs` is `wisp::main!();`; everything else is generated
//! from `src/routes` by `wisp-build`.

mod cx;
mod dev;
mod html;
mod http;

pub use cx::{Cx, Form, Method};
pub use wisp_macros::action;

use std::borrow::Cow;
use std::fmt;
use std::future::Future;
use std::net::SocketAddr;

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub mod prelude {
    pub use crate::{Cx, Error, OrStatus, Response, Result, action, error, redirect};
}

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
/// for a `main` of your own: `wisp::app!(); fn main() { setup(); wisp::run::<App>(); }`
#[macro_export]
macro_rules! app {
    () => {
        #[doc(hidden)]
        mod __wisp {
            include!(concat!(env!("OUT_DIR"), "/wisp.rs"));
        }
        use __wisp::App;
    };
}

/// Serves the app on `$HOST:$PORT`. Defaults to port 3000 on 127.0.0.1 in
/// debug builds and 0.0.0.0 in release builds.
///
/// Runs one single-threaded tokio runtime per CPU (`$WISP_THREADS` sets the
/// count) and spreads connections across them; handlers can use tokio
/// normally (`tokio::spawn` runs on the handler's own thread).
pub fn run<A: App>() {
    let threads = std::env::var("WISP_THREADS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()));
    if let Err(e) = http::run::<A>(address(), threads) {
        eprintln!("wisp: {e}");
        std::process::exit(1);
    }
}

/// Serves the app on `addr` from inside a tokio runtime you own, for apps
/// that need to set one up themselves. [`run`] is faster.
pub async fn serve<A: App>(addr: SocketAddr) -> std::io::Result<()> {
    http::serve::<A>(addr).await
}

/// `$HOST:$PORT`, with the defaults described in [`run`].
pub fn address() -> SocketAddr {
    let host = std::env::var("HOST").unwrap_or_else(|_| if cfg!(debug_assertions) { "127.0.0.1" } else { "0.0.0.0" }.into());
    let port: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(3000);
    format!("{host}:{port}").parse().unwrap_or_else(|_| panic!("invalid HOST/PORT: {host}:{port}"))
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
    fn shell() -> [&'static str; 3];
    fn asset(path: &str) -> Option<&'static Asset>;
    fn handle(route: usize, cx: &mut Cx, out: &mut Out) -> impl Future<Output = Result<()>> + Send;
    fn error(route: Option<usize>, cx: &mut Cx, out: &mut Out, status: u16, message: &str) -> impl Future<Output = Result<()>> + Send;
}

/// Where a request's output goes: HTML for the head and body of the shell,
/// or a complete response from a `+server.rs` endpoint.
#[derive(Default)]
pub struct Out {
    pub head: String,
    pub body: String,
    response: Option<Response>,
}

impl Out {
    fn clear(&mut self) {
        self.head.clear();
        self.body.clear();
        self.response = None;
    }
}

/// A file embedded in a release binary.
pub struct Asset {
    pub body: &'static [u8],
    pub ext: &'static str,
    pub etag: &'static str,
}

/// A complete response from a `+server.rs` endpoint.
pub struct Response {
    pub status: u16,
    pub content_type: Cow<'static, str>,
    pub headers: Vec<(Cow<'static, str>, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(content_type: impl Into<Cow<'static, str>>, body: impl Into<Vec<u8>>) -> Response {
        Response { status: 200, content_type: content_type.into(), headers: Vec::new(), body: body.into() }
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

    pub fn with_status(mut self, status: u16) -> Response {
        assert!((100..=999).contains(&status), "invalid status {status}");
        self.status = status;
        self
    }

    /// Panics on CR/LF in the value (header injection).
    pub fn with_header(mut self, name: impl Into<Cow<'static, str>>, value: impl Into<String>) -> Response {
        let (name, value) = (name.into(), value.into());
        assert!(cx::valid_header(&name, &value), "invalid header {name:?}: {value:?}");
        self.headers.push((name, value));
        self
    }
}

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
        assert!((400..=599).contains(&status), "error status must be 4xx or 5xx, got {status}");
        Error { status, message: message.into(), header: None, source: None }
    }

    /// A redirect with a status other than [`redirect`]'s 303, such as 308
    /// for a page that moved for good. Panics on CR/LF in `location`.
    pub fn redirect(status: u16, location: impl Into<String>) -> Error {
        let location = location.into();
        assert!((300..=308).contains(&status), "redirect status must be 3xx, got {status}");
        assert!(cx::valid_header("location", &location), "invalid redirect location {location:?}");
        Error { status, message: Cow::Borrowed(""), header: Some(("location", location)), source: None }
    }

    pub fn status(&self) -> u16 {
        self.status
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

/// `Err(error(404, "No such post"))?`
pub fn error(status: u16, message: impl Into<Cow<'static, str>>) -> Error {
    Error::new(status, message)
}

/// `return Err(redirect("/login"))`: 303 See Other, which sends the browser
/// to `location` with a GET, whether it came with a form post or a link.
/// [`Error::redirect`] takes other statuses. Panics on CR/LF in `location`.
pub fn redirect(location: impl Into<String>) -> Error {
    Error::redirect(303, location)
}

impl<E: std::error::Error + Send + Sync + 'static> From<E> for Error {
    fn from(e: E) -> Error {
        Error { status: 500, message: Cow::Owned(e.to_string()), header: None, source: Some(Box::new(e)) }
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
    pub use crate::cx::MAX_PARAMS;
    pub use crate::dev::chunk;
    pub use crate::html::{escape, raw as html, text};
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

    /// Form posts must come from our own origin (CSRF). Browsers always send
    /// `Origin` on POST; non-browser clients without it are allowed.
    pub fn check_origin(cx: &Cx) -> crate::Result<()> {
        let (Some(origin), Some(host)) = (cx.header("origin"), cx.header("host")) else { return Ok(()) };
        let origin_host = origin.split_once("://").map_or(origin, |(_, h)| h);
        if origin_host.eq_ignore_ascii_case(host) {
            Ok(())
        } else {
            Err(Error::new(403, "Cross-site form submissions are forbidden"))
        }
    }

    pub fn no_action(name: &str) -> Error {
        Error::new(404, format!("No action named `{name}` on this page"))
    }

    pub fn method_not_allowed(allow: &'static str) -> Error {
        Error { status: 405, message: "Method Not Allowed".into(), header: Some(("allow", allow.into())), source: None }
    }

    /// Used when no `+error.wisp` applies, or when rendering one failed. It
    /// says what happened, the reason, and the status with the request as a
    /// reference line. Its styles are scoped to `.wisp-error`, and it
    /// brings them along, since the app's own CSS may not exist yet.
    pub fn default_error(cx: &Cx, out: &mut Out, status: u16, message: &str) {
        let title = match crate::http::reason(status) {
            "" => "Something Went Wrong",
            reason => reason,
        };
        out.head.push_str("<title>");
        text(&mut out.head, title);
        out.head.push_str("</title><style>");
        out.head.push_str(DEFAULT_ERROR_CSS);
        out.head.push_str("</style>");

        out.body.push_str("<main class=\"wisp-error\"><p class=\"wisp-error-overline\">Error ");
        text(&mut out.body, &status);
        out.body.push_str("</p><h1>");
        text(&mut out.body, title);
        out.body.push_str("</h1>");
        if !message.eq_ignore_ascii_case(title) {
            out.body.push_str("<p class=\"wisp-error-message\">");
            text(&mut out.body, message);
            out.body.push_str("</p>");
        }
        out.body.push_str("<p class=\"wisp-error-ref\">");
        text(&mut out.body, &status);
        out.body.push_str(" · ");
        text(&mut out.body, cx.method.as_str());
        out.body.push(' ');
        text(&mut out.body, cx.path());
        out.body.push_str("</p><a href=\"/\">Go to the Home Page</a></main>");
    }

    const DEFAULT_ERROR_CSS: &str = "\
:root{color-scheme:dark}\
body{margin:0;background:#141414}\
.wisp-error{box-sizing:border-box;width:min(28rem,100% - 2rem);margin:16vh auto 0;padding:1.5rem;\
border:1px solid #4b4b4b;border-radius:.5rem;background:#1f1f1f;color:#fafafa;\
box-shadow:0 1px 2px rgb(0 0 0/.22),0 6px 16px -4px rgb(0 0 0/.17);\
font:400 .875rem/1.25rem \"Open Sans Variable\",\"Open Sans\",\"Segoe UI Variable\",\"Segoe UI\",-apple-system,BlinkMacSystemFont,system-ui,sans-serif}\
.wisp-error p{margin:0}\
.wisp-error-overline{color:#949494;font-size:.6875rem;font-weight:600;letter-spacing:.025em;line-height:1rem;text-transform:uppercase}\
.wisp-error h1{margin:.25rem 0 0;font-size:1.125rem;font-weight:600;letter-spacing:-.025em;line-height:1.5rem}\
.wisp-error .wisp-error-message{margin-top:.25rem;color:#a8a8a8}\
.wisp-error .wisp-error-ref{margin-top:1rem;color:#949494;font:400 .6875rem/1rem \"Cascadia Code\",\"JetBrains Mono\",ui-monospace,SFMono-Regular,Menlo,monospace}\
.wisp-error a{display:inline-flex;align-items:center;height:2.25rem;margin-top:1rem;padding:0 .75rem;\
border:1px solid #4b4b4b;border-radius:.375rem;background:#1f1f1f;color:#fafafa;font-weight:500;text-decoration:none;\
box-shadow:0 1px 2px rgb(0 0 0/.22);transition:background-color 250ms cubic-bezier(.25,1,.5,1),border-color 250ms cubic-bezier(.25,1,.5,1)}\
.wisp-error a:hover{border-color:#6f6f6f;background:#2e2e2e}\
.wisp-error a:active{background-image:linear-gradient(rgb(250 250 250/.08) 0 0)}\
.wisp-error a:focus-visible{outline:2px solid #896ce0;outline-offset:2px}";
}
