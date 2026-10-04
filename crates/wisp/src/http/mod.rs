//! HTTP/1.1 server.
//!
//! One task per connection. Each connection owns a `Cx` (which owns the read
//! buffer), a write buffer and an `Out`, all reused across requests, so a
//! warm connection allocates nothing for a typical page. Requests are parsed
//! in place and every complete request in the read buffer is answered
//! before a single write, which gives pipelining for free.
//!
//! Deliberately not here: TLS, HTTP/2, compression of pages (embedded files
//! are gzipped once, `compress.rs`). A reverse proxy or CDN does those better.
//!
//! The edge build (wasm32) has no sockets: it keeps `handle` and what it
//! needs, and leaves the server out.
#![cfg_attr(target_arch = "wasm32", allow(dead_code, unused_imports))]

use crate::cx::{Cx, KNOWN, Known, Method, Span, decode, hex_digit, valid_header};
#[cfg(not(target_arch = "wasm32"))]
use crate::policy::READ_CAPACITY;
use crate::policy::{self, KEEP_CAPACITY, WRITE_TIMEOUT};
use crate::{App, Error, Out, dev, rt, swar};
use std::borrow::Cow;
use std::cell::Cell;
use std::future::Future;
use std::io::{self, Write};
use std::mem::MaybeUninit;
use std::net::SocketAddr;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::task::Poll;
use std::time::{Duration, Instant};
#[cfg(not(target_arch = "wasm32"))]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(not(target_arch = "wasm32"))]
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, mpsc};

/// Headers a request may have: as many as hyper's, room for a browser's
/// behind a proxy or two that add their own.
const MAX_HEADERS: usize = 100;
const MAX_HEAD: usize = 16 * 1024;
/// Time a stopping server waits for the requests under way.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);
/// A handler that holds its thread longer than this in one go is reported
/// in dev: every other connection on that thread waited meanwhile.
const BLOCKING: Duration = Duration::from_millis(100);

/// The browser runtime: as written in dev builds, without comments and
/// indentation in release ones (see `build.rs`). The ETag tells them apart.
#[cfg(debug_assertions)]
const CLIENT_JS: &[u8] = wisp_shared::WISP_JS.as_bytes();
#[cfg(not(debug_assertions))]
const CLIENT_JS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/wisp.js"));
#[cfg(debug_assertions)]
const CLIENT_JS_ETAG: &str = concat!("\"", env!("WISP_RUNTIME_V"), "-dev\"");
#[cfg(not(debug_assertions))]
const CLIENT_JS_ETAG: &str = concat!("\"", env!("WISP_RUNTIME_V"), "\"");
/// The runtime of client scripts and directives, linked by pages that
/// render any (see `live.rs`). Versioned like `wisp.js`.
#[cfg(debug_assertions)]
const LIVE_JS: &[u8] = wisp_shared::LIVE_JS.as_bytes();
#[cfg(not(debug_assertions))]
const LIVE_JS: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/live.js"));
/// Live reload and the build error dialog, with the dialog's styles. Served
/// and linked only by debug builds, so none of it ships in a release
/// binary's pages.
#[cfg(not(target_arch = "wasm32"))]
const DEV_JS: &[u8] = include_bytes!("../client/wisp-dev.js");
/// The edge build has no dev mode to serve them to: 12 KB less in the wasm.
#[cfg(target_arch = "wasm32")]
const DEV_JS: &[u8] = b"";
/// The devtools overlay (`Alt+Shift+W`): debug builds only.
#[cfg(debug_assertions)]
const DEVTOOLS_JS: &[u8] = include_bytes!("../client/wisp-devtools.js");
/// Also inlined into the API docs page.
pub(crate) const UI_CSS: &str = concat!(
    include_str!("../client/tokens.css"),
    include_str!("../client/ui.css")
);
/// The tokens alone, which the plain error page starts from.
pub(crate) const TOKENS_CSS: &str = include_str!("../client/tokens.css");
/// The default error page's own styles.
pub(crate) const ERROR_CSS: &str = include_str!("../client/error.css");
#[cfg(not(target_arch = "wasm32"))]
const DIALOG_CSS: &[u8] = include_bytes!("../client/dialog.css");
#[cfg(target_arch = "wasm32")]
const DIALOG_CSS: &[u8] = b"";

/// The page at `/_wisp/docs` that lists the app's endpoints and sends
/// requests to them, with Wisp's own styles.
fn api_docs() -> &'static [u8] {
    static PAGE: OnceLock<String> = OnceLock::new();
    PAGE.get_or_init(|| include_str!("../api-docs.html").replace("/*ui.css*/", UI_CSS))
        .as_bytes()
}

/// `<link>`/`<script>` tags for `%wisp.head%`. Fixed for the process.
/// Baked pages have them as wisp-build writes them (`Project::baked`), the
/// same out of dev mode: change both.
static HEAD_TAGS: OnceLock<String> = OnceLock::new();

#[cfg(target_arch = "wasm32")]
#[path = "../edge_conn.rs"]
pub(crate) mod edge_conn;

/// HTTP/2 with prior knowledge (the `h2` feature): see `h2.rs`.
#[cfg(feature = "h2")]
#[path = "../h2.rs"]
mod h2;

mod clock;
mod conn;
mod decide;
mod files;
mod listen;
mod parse;
mod reply;
#[cfg(test)]
mod tests;

pub(crate) use clock::*;
pub(crate) use conn::*;
pub use decide::*;
pub(crate) use files::*;
pub(crate) use listen::*;
pub(crate) use parse::*;
pub use reply::*;
