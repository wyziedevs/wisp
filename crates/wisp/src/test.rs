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

use crate::{App, Body, Reply, Request};
use std::marker::PhantomData;

pub struct Client<A> {
    runtime: tokio::runtime::Runtime,
    cookies: Vec<(String, String)>,
    /// `authorization`, from [`Client::bearer`].
    auth: Option<String>,
    app: PhantomData<fn() -> A>,
}

/// A client for `A`, once [`crate::prepare`] has run. Panics if `init` fails.
pub fn client<A: App>() -> Client<A> {
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
        app: PhantomData,
    }
}

impl<A: App> Client<A> {
    pub fn get(&mut self, target: &str) -> Reply {
        self.send(Request::new("GET", target))
    }

    /// A form post, as a browser sends one: `app.post_form("/login", &[("name", "ada")])`.
    pub fn post_form(&mut self, target: &str, fields: &[(&str, &str)]) -> Reply {
        let mut req = Request::new("POST", target);
        req.header("content-type", "application/x-www-form-urlencoded");
        for (i, (name, value)) in fields.iter().enumerate() {
            if i > 0 {
                req.body.push(b'&');
            }
            encode(&mut req.body, name);
            req.body.push(b'=');
            encode(&mut req.body, value);
        }
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

    fn send_json(&mut self, method: &str, target: &str, json: &str) -> Reply {
        let mut req = Request::new(method, target);
        req.header("content-type", "application/json");
        req.body = json.as_bytes().to_vec();
        self.send(req)
    }

    /// Sends `Authorization: Bearer <token>` with every request from now on.
    pub fn bearer(&mut self, token: &str) {
        self.auth = Some(format!("Bearer {token}"));
    }

    /// Any request, with the client's cookies (and bearer token) added.
    pub fn send(&mut self, mut req: Request) -> Reply {
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

/// `application/x-www-form-urlencoded`.
fn encode(out: &mut Vec<u8>, s: &str) {
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => out.push(b),
            b' ' => out.push(b'+'),
            _ => out.extend_from_slice(format!("%{b:02X}").as_bytes()),
        }
    }
}
