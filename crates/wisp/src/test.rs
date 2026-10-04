//! Testing an app in process, with no server or port:
//!
//! ```ignore
//! #[test]
//! fn home() {
//!     let mut app = wisp::test::client::<App>();
//!     let page = app.get("/");
//!     assert_eq!(page.status, 200);
//!     assert!(page.text().contains("Welcome"));
//! }
//! ```
//!
//! Requests go through [`crate::handle`], the same path the server takes.
//! The client keeps the cookies responses set, as a browser would.
//!
//! With the `browser` feature, [`browser`] drives the app in a real
//! headless Chrome or Edge instead: `let mut b = wisp::browser!(App);`.

mod ws;

pub use ws::TestSocket;

use crate::{App, Body, Reply, Request};
use std::marker::PhantomData;

#[cfg(feature = "browser")]
mod browser;
#[cfg(feature = "browser")]
pub use browser::{Browser, browser};

/// The app in a headless browser, for a test: [`browser`]'s [`Browser`],
/// or, with no Chrome or Edge installed, a return from the test (which
/// then passes, having said why on stderr).
///
/// ```ignore
/// let mut b = wisp::browser!(App);
/// b.goto("/");
/// ```
#[cfg(feature = "browser")]
#[macro_export]
macro_rules! browser {
    ($app:ty) => {
        match $crate::test::browser::<$app>() {
            Some(b) => b,
            None => return,
        }
    };
}

impl Reply {
    /// Where a redirect goes: `assert_eq!(r.location(), Some("/login"))`.
    pub fn location(&self) -> Option<&str> {
        self.header("location")
    }
}

/// What a browser asks for when it shows a page: an error is then a page,
/// where a request with no `Accept` is taken for an API client's.
const PAGE: &str = "text/html,*/*;q=0.8";

/// A test client over an app: calls routes in process, with cookies kept between calls. Made by `wisp::test::client`.
pub struct Client<A> {
    runtime: tokio::runtime::Runtime,
    cookies: Vec<(String, String)>,
    /// `authorization`, from [`Client::bearer`].
    auth: Option<String>,
    /// Headers for the next request only, from [`Client::header`].
    next: Vec<(String, String)>,
    app: PhantomData<fn() -> A>,
}

/// Empties every table, so the next test starts from nothing; rows are
/// gone and ids not given again, as after `Table::clear`.
pub fn fresh() {
    crate::table::wipe_all();
}

/// A client for `A`, once [`crate::prepare`] has run. Panics if `init` fails.
///
/// Durable tables stay in memory in tests (each test process starts
/// empty), unless `init` gives a store of its own with `wisp::store`.
pub fn client<A: App>() -> Client<A> {
    crate::store::memory();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a tokio runtime for the test client");
    if let Err(e) = runtime.block_on(crate::prepare::<A>()) {
        panic!("{e}");
    }
    Client {
        runtime,
        cookies: Vec::new(),
        auth: None,
        next: Vec::new(),
        app: PhantomData,
    }
}

impl<A: App> Client<A> {
    /// A `GET` of `target`, such as `"/about"`; `Reply` is what came back.
    pub fn get(&mut self, target: &str) -> Reply {
        let mut req = Request::new("GET", target);
        req.header("accept", PAGE);
        self.send(req)
    }

    /// A form post, as a browser sends one: `app.post_form("/login", &[("name", "ada")])`.
    pub fn post_form(&mut self, target: &str, fields: &[(&str, &str)]) -> Reply {
        // As browsers encode forms: `+` for a space.
        let encode = |out: &mut String, s: &str| {
            for (k, word) in s.split(' ').enumerate() {
                if k > 0 {
                    out.push('+');
                }
                let _ = crate::cx::encode(out, word, |b| {
                    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'*')
                });
            }
        };
        let mut req = Request::new("POST", target);
        req.header("accept", PAGE);
        req.header("content-type", "application/x-www-form-urlencoded");
        let mut body = String::new();
        for (i, (name, value)) in fields.iter().enumerate() {
            if i > 0 {
                body.push('&');
            }
            encode(&mut body, name);
            body.push('=');
            encode(&mut body, value);
        }
        req.body = body.into_bytes();
        self.send(req)
    }

    /// A file upload, as a browser sends a form with one file:
    /// `app.upload("/avatar", "photo", "image/png", &bytes)`.
    pub fn upload(&mut self, target: &str, field: &str, mime: &str, bytes: &[u8]) -> Reply {
        const BOUNDARY: &str = "----wisp-test-boundary";
        let mut req = Request::new("POST", target);
        req.header("accept", PAGE);
        req.header(
            "content-type",
            &format!("multipart/form-data; boundary={BOUNDARY}"),
        );
        let mut body = format!(
            "--{BOUNDARY}\r\ncontent-disposition: form-data; name=\"{field}\"; filename=\"upload\"\r\ncontent-type: {mime}\r\n\r\n"
        )
        .into_bytes();
        body.extend_from_slice(bytes);
        body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
        req.body = body;
        self.send(req)
    }

    /// Signs the client in as row `id` of the app's users, as `cx.sign_in(id)`
    /// does, with no login form: every request from now on carries the session.
    pub fn sign_in(&mut self, id: u64) {
        let peer = "127.0.0.1:1".parse().expect("a socket address");
        let mut cx = crate::Cx::from_request::<A>("GET", "/", std::iter::empty(), &[], peer)
            .unwrap_or_else(|s| panic!("sign_in: a request for / was refused with {s}"));
        cx.sign_in(id);
        let set = cx.out_headers().iter().find(|(n, _)| n == "set-cookie");
        let cookie = set.and_then(|(_, v)| v.split(';').next()?.split_once('='));
        let (name, value) = cookie.expect("sign_in sets the session cookie");
        self.cookies.retain(|(n, _)| n != name);
        self.cookies.push((name.to_string(), value.to_string()));
    }

    /// The browser modules a page loads, each with its source: every
    /// `/_app/` script the page names (but the runtime's own), fetched.
    pub fn modules(&mut self, page: &Reply) -> Vec<(String, String)> {
        let html = page.text();
        let (mut urls, mut rest) = (Vec::<String>::new(), html);
        while let Some(i) = rest.find(crate::protocol::APP_PREFIX) {
            let tail = &rest[i..];
            let end = tail
                .find(['"', '\'', '?', '<', '>', ' ', '\\', ')'])
                .unwrap_or(tail.len());
            let url = &tail[..end];
            if url.ends_with(".js") && !urls.iter().any(|u| u == url) {
                urls.push(url.to_string());
            }
            rest = &tail[end..];
        }
        urls.retain(|u| !u.ends_with("/wisp.js") && !u.ends_with("/live.js"));
        urls.into_iter()
            .map(|u| {
                let source = self.get(&u).text().to_string();
                (u, source)
            })
            .collect()
    }

    /// Opens a WebSocket to a `+server.rs` that answers
    /// [`Response::websocket`](crate::Response::websocket), with the
    /// client's cookies; the handshake is checked as the server checks it.
    /// Panics if the route does not upgrade.
    pub fn websocket(&mut self, target: &str) -> TestSocket {
        let mut req = Request::new("GET", target);
        for (n, v) in [
            ("upgrade", "websocket"),
            ("connection", "Upgrade"),
            ("sec-websocket-version", "13"),
            ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
        ] {
            req.header(n, v);
        }
        let (reply, upgrade) = self.exchange(req);
        match upgrade {
            // The 501 is what `handle` makes of an upgrade; any other
            // status is the handshake's refusal.
            Some(upgrade) if reply.status == 501 => TestSocket::open(upgrade, target),
            _ => panic!(
                "websocket {target}: expected an upgrade, got {} {}",
                reply.status,
                reply.text()
            ),
        }
    }

    /// A JSON request, as an API client sends one:
    /// `app.post_json("/api/notes", r#"{"title": "Tea"}"#)`.
    pub fn post_json(&mut self, target: &str, json: &str) -> Reply {
        self.send_json("POST", target, json)
    }

    /// A `PUT` with a JSON body.
    pub fn put_json(&mut self, target: &str, json: &str) -> Reply {
        self.send_json("PUT", target, json)
    }

    /// A `PATCH` with a JSON body.
    pub fn patch_json(&mut self, target: &str, json: &str) -> Reply {
        self.send_json("PATCH", target, json)
    }

    /// A `DELETE` of `target`.
    pub fn delete(&mut self, target: &str) -> Reply {
        self.send(Request::new("DELETE", target))
    }

    /// A request of any method with a JSON body.
    pub fn send_json(&mut self, method: &str, target: &str, json: &str) -> Reply {
        let mut req = Request::new(method, target);
        req.header("content-type", "application/json");
        req.body = json.as_bytes().to_vec();
        self.send(req)
    }

    /// Sends `Authorization: Bearer <token>` with every request from now on.
    pub fn bearer(&mut self, token: &str) {
        self.auth = Some(format!("Bearer {token}"));
    }

    /// Sends a header with the next request only:
    /// `app.header("if-match", &etag); app.put_json(..)`.
    pub fn header(&mut self, name: &str, value: &str) {
        self.next.push((name.into(), value.into()));
    }

    /// Any request, with the client's cookies (and bearer token) added.
    pub fn send(&mut self, req: Request) -> Reply {
        self.exchange(req).0
    }

    /// [`Client::send`], and the WebSocket handler the route made, if any.
    fn exchange(&mut self, mut req: Request) -> (Reply, Option<crate::ws::Upgrade>) {
        for (n, v) in std::mem::take(&mut self.next) {
            // One given for this request wins over a default.
            req.headers.retain(|(k, _)| !k.eq_ignore_ascii_case(&n));
            req.header(&n, &v);
        }
        if let Some(auth) = &self.auth
            && !req
                .headers
                .iter()
                .any(|(n, _)| n.eq_ignore_ascii_case("authorization"))
        {
            req.header("authorization", auth);
        }
        if !self.cookies.is_empty() {
            let jar: Vec<String> = self
                .cookies
                .iter()
                .map(|(n, v)| format!("{n}={v}"))
                .collect();
            req.header("cookie", &jar.join("; "));
        }
        let mut upgrade = None;
        let reply = self
            .runtime
            .block_on(crate::http::handle_keeping::<A>(req, &mut upgrade));
        for (_, set) in reply
            .headers
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case("set-cookie"))
        {
            let Some((name, value)) = set.split(';').next().and_then(|c| c.split_once('=')) else {
                continue;
            };
            let gone = value.is_empty() || set.to_ascii_lowercase().contains("max-age=0");
            self.cookies.retain(|(n, _)| n != name);
            if !gone {
                self.cookies.push((name.to_string(), value.to_string()));
            }
        }
        (reply, upgrade)
    }

    /// The next chunk of a streamed reply; `None` once it has ended.
    pub fn next_chunk(&self, reply: &mut Reply) -> Option<Vec<u8>> {
        match &mut reply.body {
            Body::Stream(rx) => self.runtime.block_on(rx.recv()),
            _ => None,
        }
    }

    /// A cookie the client holds.
    pub fn cookie(&self, name: &str) -> Option<&str> {
        self.cookies
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}
