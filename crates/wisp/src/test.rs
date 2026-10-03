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

pub struct Client<A> {
    runtime: tokio::runtime::Runtime,
    cookies: Vec<(String, String)>,
    /// `authorization`, from [`Client::bearer`].
    auth: Option<String>,
    /// Headers for the next request only, from [`Client::header`].
    next: Vec<(String, String)>,
    app: PhantomData<fn() -> A>,
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
    pub fn get(&mut self, target: &str) -> Reply {
        self.send(Request::new("GET", target))
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

    /// A JSON request, as an API client sends one:
    /// `app.post_json("/api/notes", r#"{"title": "Tea"}"#)`.
    pub fn post_json(&mut self, target: &str, json: &str) -> Reply {
        self.send_json("POST", target, json)
    }

    pub fn put_json(&mut self, target: &str, json: &str) -> Reply {
        self.send_json("PUT", target, json)
    }

    pub fn patch_json(&mut self, target: &str, json: &str) -> Reply {
        self.send_json("PATCH", target, json)
    }

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
    pub fn send(&mut self, mut req: Request) -> Reply {
        for (n, v) in std::mem::take(&mut self.next) {
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
        let reply = self.runtime.block_on(crate::handle::<A>(req));
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
        reply
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
