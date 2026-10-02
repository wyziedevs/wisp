//! The request context passed to `load`, actions and endpoints.
//!
//! There is one `Cx` per connection. It owns the connection's read buffer
//! and describes the current request as spans into it, so nothing is copied
//! and, once warm, nothing is allocated. Owning the buffer (rather than
//! borrowing it) keeps `Cx` free of lifetimes: handlers take `&mut Cx`.

use crate::form::{Form, pairs};
use crate::{Response, sign};
use std::any::{Any, TypeId};
use std::borrow::Cow;
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::time::Duration;

pub const MAX_PARAMS: usize = 8;
/// The deepest path a `[...rest]` route matches.
pub const MAX_SEGS: usize = 32;

/// `/a/b` → `["a", "b"]`, `/` → `[]`, for the router's arms with
/// parameters (a path without any is matched whole). `None` when deeper
/// than `N`: the router's deepest arm, or `MAX_SEGS` with a `[...rest]`.
pub fn split<'a, 'b, const N: usize>(
    path: &'a str,
    segs: &'b mut [&'a str; N],
) -> Option<&'b [&'a str]> {
    if path == "/" {
        return Some(&[]);
    }
    let mut n = 0;
    for s in path.get(1..)?.split('/') {
        *segs.get_mut(n)? = s;
        n += 1;
    }
    Some(&segs[..n])
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Method {
    Get,
    Head,
    Post,
    Put,
    Patch,
    Delete,
    Options,
    Other,
}

impl Method {
    pub(crate) fn parse(s: &[u8]) -> Method {
        match s {
            b"GET" => Method::Get,
            b"HEAD" => Method::Head,
            b"POST" => Method::Post,
            b"PUT" => Method::Put,
            b"PATCH" => Method::Patch,
            b"DELETE" => Method::Delete,
            b"OPTIONS" => Method::Options,
            _ => Method::Other,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Head => "HEAD",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
            Method::Options => "OPTIONS",
            Method::Other => "OTHER",
        }
    }
}

/// A byte range of `Cx::buf`.
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct Span {
    pub start: u32,
    pub len: u32,
}

impl Span {
    /// The span of `part`, which must be a subslice of `buf` or empty (the
    /// router uses a static `""` for absent optional parameters).
    pub fn of(buf: &[u8], part: &[u8]) -> Span {
        if part.is_empty() {
            return Span::default();
        }
        let start = part.as_ptr() as usize - buf.as_ptr() as usize;
        debug_assert!(start + part.len() <= buf.len(), "span outside buffer");
        Span {
            start: start as u32,
            len: part.len() as u32,
        }
    }

    pub fn range(self) -> std::ops::Range<usize> {
        self.start as usize..(self.start + self.len) as usize
    }
}

/// The request as it came over the wire: the connection's read buffer and
/// spans into it. The parser in `http.rs` writes it; `Cx`'s methods read it.
pub(crate) struct Wire {
    /// This request and any pipelined after it.
    pub buf: Vec<u8>,
    pub path: Span,
    pub query: Span,
    pub body: Span,
    pub headers: Vec<(Span, Span)>,
    /// HTTP/1.1 rather than 1.0, which cannot take a chunked response.
    pub http11: bool,
    pub peer: SocketAddr,
}

pub struct Cx {
    pub method: Method,
    pub(crate) wire: Wire,
    names: &'static [&'static str],
    params: [Span; MAX_PARAMS],
    /// Percent-decoded copies, only for params that needed decoding.
    decoded: [Option<String>; MAX_PARAMS],
    status: u16,
    /// The response's headers, `set-cookie` among them: `cookie` reads what
    /// this request set before what it sent, so a page's `load` sees what
    /// its action just stored.
    out_headers: Vec<(Cow<'static, str>, Cow<'static, str>)>,
    /// How many of `out_headers` the `before` hook set. Those stay on an
    /// error page; a handler's are dropped with the page it did not finish.
    /// A `u32` packs it with the fields beside it.
    kept_headers: u32,
    /// Signed cookies whose signature held, as (name, cookie as read): each
    /// is checked once a request.
    verified: std::sync::Mutex<Vec<(String, String)>>,
    /// Values handed along the request with `set`, one per type.
    locals: Vec<(TypeId, Box<dyn Any + Send + Sync>)>,
    /// A JSON body, parsed once for the handler parameters read from it.
    json: std::sync::OnceLock<Option<crate::Value>>,
    /// The request id, once one is asked for (see [`Cx::request_id`]).
    id: std::sync::OnceLock<String>,
    /// Routed to a `+server.rs` endpoint, whose errors are JSON.
    api: bool,
}

impl Cx {
    pub(crate) fn new(peer: SocketAddr) -> Cx {
        Cx {
            method: Method::Get,
            wire: Wire {
                buf: Vec::with_capacity(8 * 1024),
                path: Span::default(),
                query: Span::default(),
                body: Span::default(),
                headers: Vec::with_capacity(16),
                http11: true,
                peer,
            },
            names: &[],
            params: [Span::default(); MAX_PARAMS],
            decoded: [const { None }; MAX_PARAMS],
            status: 200,
            out_headers: Vec::new(),
            kept_headers: 0,
            verified: std::sync::Mutex::new(Vec::new()),
            locals: Vec::new(),
            json: std::sync::OnceLock::new(),
            id: std::sync::OnceLock::new(),
            api: false,
        }
    }

    /// Clears per-request state; request spans are set by the parser.
    pub(crate) fn reset(&mut self) {
        // Only the route's own are ever set (`set_params`).
        self.decoded[..self.names.len()].fill(None);
        self.names = &[];
        self.status = 200;
        self.out_headers.clear();
        self.kept_headers = 0;
        self.verified
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.locals.clear();
        self.json.take();
        self.id.take();
        self.api = false;
    }

    /// The body parsed as JSON, once per request; `None` if it is not JSON.
    pub(crate) fn json_body(&self) -> Option<&crate::Value> {
        self.json
            .get_or_init(|| crate::json::parse(std::str::from_utf8(self.body()).ok()?).ok())
            .as_ref()
    }

    /// Whether the body was parsed as JSON (see [`Cx::json_body`]) and was
    /// not JSON: a load, for the error path to ask.
    pub(crate) fn json_failed(&self) -> bool {
        matches!(self.json.get(), Some(None))
    }

    pub(crate) fn set_params(&mut self, names: &'static [&'static str], spans: [Span; MAX_PARAMS]) {
        debug_assert!(names.len() <= MAX_PARAMS);
        self.names = names;
        self.params = spans;
        // The spans are of the path, a `str`: only an escape makes one owned.
        for (span, decoded) in spans.iter().zip(&mut self.decoded).take(names.len()) {
            let raw = &self.wire.buf[span.range()];
            *decoded = match raw.contains(&b'%') {
                true => Some(decode(raw, false).into_owned()),
                false => None,
            };
        }
    }

    fn str(&self, span: Span) -> &str {
        // Request lines and header names are validated ASCII by httparse.
        std::str::from_utf8(&self.wire.buf[span.range()]).unwrap_or("")
    }

    /// The URL path, as sent (not percent-decoded).
    pub fn path(&self) -> &str {
        self.str(self.wire.path)
    }

    /// The raw query string, without the `?`.
    pub fn query_string(&self) -> &str {
        self.str(self.wire.query)
    }

    /// A route parameter such as `slug` in `blog/[slug]`, percent-decoded.
    /// Optional parameters that are absent are `""`.
    ///
    /// Panics if the route has no such parameter: that is a typo in code,
    /// not bad input.
    pub fn param(&self, name: &str) -> &str {
        match self.route_param(name) {
            Some(v) => v,
            None => panic!("route has no parameter `{name}` (it has {:?})", self.names),
        }
    }

    /// The route parameter `name`, if the route has one.
    pub(crate) fn route_param(&self, name: &str) -> Option<&str> {
        let i = self.names.iter().position(|n| *n == name)?;
        Some(
            self.decoded[i]
                .as_deref()
                .unwrap_or_else(|| self.str(self.params[i])),
        )
    }

    /// First query parameter named `name`, decoded.
    pub fn query(&self, name: &str) -> Option<Cow<'_, str>> {
        pairs(&self.wire.buf[self.wire.query.range()])
            .find(|(k, _)| k == name)
            .map(|(_, v)| v)
    }

    /// Every query parameter, decoded, in order.
    pub(crate) fn query_pairs(&self) -> impl Iterator<Item = (Cow<'_, str>, Cow<'_, str>)> {
        pairs(&self.wire.buf[self.wire.query.range()])
    }

    /// The route's parameters, decoded, as (name, value).
    pub(crate) fn params(&self) -> impl Iterator<Item = (&'static str, &str)> {
        self.names
            .iter()
            .map(|&n| (n, self.route_param(n).unwrap_or("")))
    }

    /// Every query parameter named `name`, decoded, in order.
    pub(crate) fn query_all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = Cow<'a, str>> {
        pairs(&self.wire.buf[self.wire.query.range()])
            .filter(move |(k, _)| k == name)
            .map(|(_, v)| v)
    }

    /// A query parameter parsed as any `FromStr` type, or `default` when it
    /// is missing or does not parse: `let page: u32 = cx.query_or("page", 1);`
    pub fn query_or<T: FromStr>(&self, name: &str, default: T) -> T {
        parsed_or(self.query(name).as_deref(), default)
    }

    /// The form the request carries, urlencoded or multipart (which is how
    /// forms send files: [`Form::file`]). Empty for any other body.
    pub fn form(&self) -> Form<'_> {
        Form::new(self.header("content-type"), self.body())
    }

    /// The request body as sent, whatever its type: JSON for
    /// `serde_json::from_slice(cx.body())`, say.
    pub fn body(&self) -> &[u8] {
        &self.wire.buf[self.wire.body.range()]
    }

    /// Request header by case-insensitive name. Non-UTF-8 values are `None`.
    pub fn header(&self, name: &str) -> Option<&str> {
        let (_, v) = self
            .wire
            .headers
            .iter()
            .find(|(n, _)| self.wire.buf[n.range()].eq_ignore_ascii_case(name.as_bytes()))?;
        std::str::from_utf8(&self.wire.buf[v.range()]).ok()
    }

    /// A header parsed as any `FromStr` type, or `default` when it is
    /// missing or does not parse: `let v: u32 = cx.header_or("x-api-version", 1);`
    pub fn header_or<T: FromStr>(&self, name: &str, default: T) -> T {
        parsed_or(self.header(name).map(str::trim), default)
    }

    /// The body's media type, its `content-type` without parameters:
    /// `application/json`. Empty when it has none.
    pub(crate) fn mime(&self) -> &str {
        let t = self.header("content-type").unwrap_or("");
        t.split(';').next().unwrap_or("").trim()
    }

    /// The first value of a header each proxy adds to, such as
    /// `x-forwarded-proto`: what the proxy nearest the client saw.
    pub(crate) fn forwarded(&self, name: &str) -> Option<&str> {
        self.header(name)?.split(',').next().map(str::trim)
    }

    /// An id for this request: the client's `x-request-id` when it sent a
    /// sane one (1 to 128 visible ASCII characters), else a new one. A
    /// request that asks is answered with it as `x-request-id`, so a log line
    /// here and a bug report there name the same request. `WISP_REQUEST_ID=on`
    /// gives every request one, and puts it in the dev log. Nothing is made
    /// or sent for requests that never ask.
    pub fn request_id(&self) -> &str {
        self.id.get_or_init(|| match self.header("x-request-id") {
            Some(v) if (1..=128).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_graphic()) => {
                v.to_string()
            }
            _ => new_id(),
        })
    }

    /// The id, if the request has asked for one.
    pub(crate) fn id(&self) -> Option<&str> {
        self.id.get().map(String::as_str)
    }

    /// The `Host` header: `example.com` or `localhost:3000`.
    pub fn host(&self) -> Option<&str> {
        self.header("host")
    }

    /// The token of an `Authorization: Bearer <token>` header, for APIs.
    pub fn bearer(&self) -> Option<&str> {
        let v = self.header("authorization")?;
        let (scheme, token) = v.split_once(' ')?;
        let token = token.trim();
        (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
    }

    /// Unless the request has `Authorization: Bearer <token>` with the
    /// token in the environment variable `var` (compared in constant
    /// time), a 401: `cx.need_bearer("API_KEY")?`. Unset is never matched.
    pub fn need_bearer(&self, var: &str) -> crate::Result {
        let sent = self.bearer().unwrap_or("");
        if sign::secret(var, |k| crate::secure_eq(sent, &k.bytes)) == Some(true) {
            return Ok(());
        }
        Err(crate::Error::new(401, "Unauthorized").with_header("www-authenticate", "Bearer"))
    }

    /// Whether the request may change something: any method but GET, HEAD
    /// and OPTIONS. `if cx.writes() { cx.need_bearer("API_KEY")?; }`
    pub fn writes(&self) -> bool {
        !matches!(self.method, Method::Get | Method::Head | Method::Options)
    }

    /// The user name and password of an `Authorization: Basic` header, as
    /// `curl -u name:password` sends them. Compare the password with
    /// [`crate::secure_eq`].
    pub fn basic_auth(&self) -> Option<(String, String)> {
        let v = self.header("authorization")?;
        let (scheme, encoded) = v.split_once(' ')?;
        if !scheme.eq_ignore_ascii_case("basic") {
            return None;
        }
        let encoded = encoded.trim();
        let mut raw = vec![0; encoded.len() * 3 / 4];
        let len = sign::unbase64(encoded, &mut raw)?;
        raw.truncate(len);
        let text = String::from_utf8(raw).ok()?;
        let (name, password) = text.split_once(':')?;
        Some((name.to_string(), password.to_string()))
    }

    /// Lets pages on other sites call the app from the browser (CORS):
    /// `origins` is `*` for any site, or the sites allowed, separated by
    /// spaces or commas (`"https://app.example.com https://example.com"`),
    /// which may also send their cookies. Call it from `before` in
    /// `src/hooks.rs` and return what it returns, which answers the
    /// browser's preflight (the OPTIONS it sends before a request that is
    /// not a plain form post):
    ///
    /// ```ignore
    /// fn before(cx: &mut Cx) -> Option<Response> {
    ///     cx.cors("*")
    /// }
    /// ```
    ///
    /// A request from a site not allowed gets no CORS headers, so its
    /// browser does not hand it the answer.
    pub fn cors(&mut self, origins: &str) -> Option<Response> {
        let origin = self.header("origin")?;
        let listed =
            |o: &str| !o.is_empty() && o.trim_end_matches('/').eq_ignore_ascii_case(origin);
        let allow = if origins.trim() == "*" {
            None
        } else if origins.split([',', ' ']).any(listed) {
            Some(origin.to_string())
        } else {
            self.put("vary", Cow::Borrowed("origin"));
            return None;
        };
        self.put("vary", Cow::Borrowed("origin"));
        match allow {
            None => self.put("access-control-allow-origin", Cow::Borrowed("*")),
            Some(origin) => {
                self.put("access-control-allow-origin", Cow::Owned(origin));
                self.put("access-control-allow-credentials", Cow::Borrowed("true"));
            }
        }
        let method = self.header("access-control-request-method")?;
        if self.method != Method::Options {
            return None;
        }
        let mut preflight = Response::empty(204)
            .with_header("access-control-allow-methods", method)
            .with_header("access-control-max-age", "86400");
        if let Some(h) = self.header("access-control-request-headers") {
            preflight = preflight.with_header("access-control-allow-headers", h);
        }
        Some(preflight)
    }

    /// Every request header as `(name, value)`, in the order sent. Values
    /// that are not UTF-8 are left out.
    pub fn headers(&self) -> impl Iterator<Item = (&str, &str)> {
        self.wire.headers.iter().filter_map(|(n, v)| {
            Some((
                std::str::from_utf8(&self.wire.buf[n.range()]).ok()?,
                std::str::from_utf8(&self.wire.buf[v.range()]).ok()?,
            ))
        })
    }

    /// A cookie's value: the one this request set with `set_cookie`, else
    /// the one the browser sent. A deleted cookie is `None`.
    pub fn cookie(&self, name: &str) -> Option<&str> {
        let set = self.out_headers.iter().rev().find_map(|(n, v)| {
            let (k, rest) = v.split_once('=')?;
            (k == name && n.eq_ignore_ascii_case("set-cookie"))
                .then(|| rest.split(';').next().unwrap_or(""))
        });
        if let Some(value) = set {
            return (!value.is_empty()).then_some(value);
        }
        let all = self.header("cookie")?;
        all.split(';').find_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            (k.trim() == name).then(|| v.trim().trim_matches('"'))
        })
    }

    /// A cookie parsed as any `FromStr` type, or `default` when it is missing
    /// or does not parse: `let count: i64 = cx.cookie_or("count", 0);`. A
    /// struct with `#[derive(Cookie)]` reads back the same way.
    pub fn cookie_or<T: FromStr>(&self, name: &str, default: T) -> T {
        parsed_or(self.cookie(name), default)
    }

    /// A cookie set with [`Cx::set_signed_cookie`], if its signature holds.
    /// A visitor can read it but cannot make one up or change it, so it can
    /// say who is signed in. Anything else of that name is `None`.
    pub fn signed_cookie(&self, name: &str) -> Option<&str> {
        let raw = self.cookie(name)?;
        let (value, mac) = raw.rsplit_once('.')?;
        let mut verified = self.verified.lock().unwrap_or_else(|e| e.into_inner());
        if !verified.iter().any(|(n, r)| n == name && r == raw) {
            if !sign::verify_cookie(name, value, mac) {
                return None;
            }
            verified.push((name.to_owned(), raw.to_owned()));
        }
        Some(value)
    }

    /// [`Cx::signed_cookie`] parsed as any `FromStr` type, or `default`.
    pub fn signed_cookie_or<T: FromStr>(&self, name: &str, default: T) -> T {
        parsed_or(self.signed_cookie(name), default)
    }

    /// The TCP peer: the client, or the proxy in front of the app.
    pub fn peer(&self) -> SocketAddr {
        self.wire.peer
    }

    /// The client's IP address. Behind a proxy, set `WISP_CLIENT_IP_HEADER`
    /// to the header it puts the address in (`x-forwarded-for`, whose last
    /// entry the proxy added, or `x-real-ip`, `cf-connecting-ip`...). Without
    /// it, or when that header holds no address, this is the peer's address:
    /// a header any client can send is never trusted by default.
    pub fn client_ip(&self) -> IpAddr {
        let from_proxy = crate::settings()
            .client_ip_header
            .as_deref()
            .and_then(|h| self.header(h))
            .and_then(|v| v.rsplit(',').next()?.trim().parse().ok());
        from_proxy.unwrap_or(self.wire.peer.ip())
    }

    /// Status for a rendered page. Endpoints set it on their `Response`.
    pub fn set_status(&mut self, status: u16) {
        assert!((100..=999).contains(&status), "invalid status {status}");
        self.status = status;
    }

    /// Adds a response header. Panics on CR/LF, which would allow header
    /// injection; building a header from unchecked input is a bug.
    pub fn set_header(&mut self, name: impl Into<Cow<'static, str>>, value: impl Into<String>) {
        let (name, value) = (name.into(), value.into());
        assert!(
            valid_header(&name, &value),
            "invalid header {name:?}: {value:?}"
        );
        self.out_headers.push((name, Cow::Owned(value)));
    }

    /// Adds a response header whose value is known to be one.
    pub(crate) fn put(&mut self, name: &'static str, value: Cow<'static, str>) {
        self.out_headers.push((Cow::Borrowed(name), value));
    }

    // The response so far, as the server reads it: the status, the headers
    // and whether errors go back as JSON. App code sets them with the
    // methods above.

    /// The status for a rendered page (see [`Cx::set_status`]).
    #[inline]
    pub(crate) fn status(&self) -> u16 {
        self.status
    }

    /// Routed to a `+server.rs` endpoint, whose errors are JSON.
    #[inline]
    pub(crate) fn api(&self) -> bool {
        self.api
    }

    #[inline]
    pub(crate) fn set_api(&mut self) {
        self.api = true;
    }

    /// The `before` hook has run: the headers set so far stay on an error
    /// page.
    #[inline]
    pub(crate) fn keep_headers(&mut self) {
        self.kept_headers = self.out_headers.len() as u32;
    }

    /// The headers the handler set, after the `before` hook's.
    #[inline]
    pub(crate) fn page_headers(&self) -> &[(Cow<'static, str>, Cow<'static, str>)] {
        &self.out_headers[self.kept_headers as usize..]
    }

    /// Takes [`Cx::page_headers`] out.
    #[inline]
    pub(crate) fn take_page_headers(
        &mut self,
    ) -> std::vec::Drain<'_, (Cow<'static, str>, Cow<'static, str>)> {
        self.out_headers.drain(self.kept_headers as usize..)
    }

    /// Drops [`Cx::page_headers`], with the page that failed.
    #[inline]
    pub(crate) fn drop_page_headers(&mut self) {
        self.out_headers.truncate(self.kept_headers as usize);
    }

    /// Moves every header set onto the end of `to`.
    #[inline]
    pub(crate) fn send_headers(&mut self, to: &mut Vec<(Cow<'static, str>, Cow<'static, str>)>) {
        to.append(&mut self.out_headers);
    }

    #[cfg(test)]
    pub(crate) fn out_headers(&self) -> &[(Cow<'static, str>, Cow<'static, str>)] {
        &self.out_headers
    }

    /// Keeps `value` for the rest of this request, for any handler to read
    /// with [`Cx::get`]: `before` in `src/hooks.rs` finds the signed-in user
    /// once, and every page reads it. One value per type; a second replaces
    /// the first.
    pub fn set<T: Any + Send + Sync>(&mut self, value: T) {
        let id = TypeId::of::<T>();
        self.locals.retain(|(t, _)| *t != id);
        self.locals.push((id, Box::new(value)));
    }

    /// The value of type `T` this request was given with [`Cx::set`].
    pub fn get<T: Any>(&self) -> Option<&T> {
        let id = TypeId::of::<T>();
        self.locals.iter().find(|(t, _)| *t == id)?.1.downcast_ref()
    }

    /// Takes the value of type `T` this request was given with [`Cx::set`]
    /// out of it, so it need not be `Clone`.
    pub fn take<T: Any>(&mut self) -> Option<T> {
        let id = TypeId::of::<T>();
        let at = self.locals.iter().position(|(t, _)| *t == id)?;
        let (_, value) = self.locals.swap_remove(at);
        value.downcast().ok().map(|v| *v)
    }

    /// A form that did not pass: the page renders with `status` (422, say),
    /// and its `load`, which runs next, reads `problem` with [`Cx::get`]:
    /// `cx.fail(422, Problem("Choose a photo"))`.
    pub fn fail<T: Any + Send + Sync>(&mut self, status: u16, problem: T) {
        self.set_status(status);
        self.set(problem);
    }

    /// A message for the next page this visitor sees, which reads it with
    /// [`Cx::flashed`]: `cx.flash("Saved"); redirect("/")`. It waits in a
    /// cookie until then, or until the browser closes.
    pub fn flash(&mut self, message: &str) {
        let options = CookieOptions {
            max_age: None,
            ..CookieOptions::default()
        };
        self.set_cookie_with(FLASH, Escape(message), options);
    }

    /// The message [`Cx::flash`] left, once: reading it deletes it.
    pub fn flashed(&mut self) -> Option<String> {
        let message = CookieReader::new(self.cookie(FLASH)?)
            .text()
            .ok()?
            .into_owned();
        self.set_cookie(FLASH, "");
        Some(message)
    }

    /// Sets a cookie for the whole site, kept for 400 days (the most browsers
    /// allow) and hidden from page scripts. The value is anything printable,
    /// such as a number or a string; an empty one deletes the cookie.
    /// [`Cx::set_cookie_with`] takes other options.
    ///
    /// Panics on a character a cookie cannot hold (space, `"`, `,`, `;`,
    /// `\`, control or non-ASCII); encode such values first.
    pub fn set_cookie(&mut self, name: &str, value: impl std::fmt::Display) {
        self.set_cookie_with(name, value, CookieOptions::default());
    }

    /// Deletes a cookie the site set: `cx.delete_cookie("user")`.
    pub fn delete_cookie(&mut self, name: &str) {
        self.set_cookie(name, "");
    }

    /// Sets a cookie a visitor cannot forge or change, read back with
    /// [`Cx::signed_cookie`]: a user id that says who is signed in, say. It
    /// is signed with `WISP_SECRET` (dev builds keep one in `.wisp/secret`).
    /// The value is still readable by the visitor; keep secrets out of it.
    pub fn set_signed_cookie(&mut self, name: &str, value: impl std::fmt::Display) {
        self.set_cookie_with(
            name,
            value,
            CookieOptions {
                signed: true,
                ..CookieOptions::default()
            },
        );
    }

    /// Sets a cookie with options other than [`Cx::set_cookie`]'s:
    /// `cx.set_cookie_with("session", id, CookieOptions { max_age: None, ..CookieOptions::default() })`
    /// for one that ends when the browser closes.
    ///
    /// It gets `Secure` when the request came over HTTPS through a proxy
    /// (`x-forwarded-proto`) or `ORIGIN` is an `https://` address, and
    /// always with `SameSite=None`, which browsers require.
    pub fn set_cookie_with(
        &mut self,
        name: &str,
        value: impl std::fmt::Display,
        options: CookieOptions,
    ) {
        use std::fmt::Write;
        let token =
            |s: &str, bad: &[u8]| s.bytes().all(|b| b.is_ascii_graphic() && !bad.contains(&b));
        assert!(
            !name.is_empty() && token(name, b"()<>@,;:\\\"/[]?={}"),
            "invalid cookie name {name:?}"
        );
        assert!(
            options.path.starts_with('/') && token(options.path, b";"),
            "invalid cookie path {:?}",
            options.path
        );
        assert!(
            options
                .domain
                .is_none_or(|d| !d.is_empty() && token(d, b";")),
            "invalid cookie domain {:?}",
            options.domain
        );
        let mut header = String::with_capacity(96);
        let _ = write!(header, "{name}={value}");
        let value = &header[name.len() + 1..];
        assert!(token(value, b"\",;\\"), "invalid cookie value {value:?}");
        let deleted = value.is_empty();
        if options.signed && !deleted {
            let mac = sign::cookie_mac(name, value);
            header.push('.');
            sign::base64(&mut header, &mac, true);
        }
        let _ = write!(header, "; Path={}", options.path);
        if let Some(domain) = options.domain {
            header.push_str("; Domain=");
            header.push_str(domain);
        }
        match options.max_age {
            _ if deleted => header.push_str("; Max-Age=0"),
            Some(age) => {
                let _ = write!(header, "; Max-Age={}", age.as_secs());
            }
            None => {}
        }
        if !options.script_readable {
            header.push_str("; HttpOnly");
        }
        if options.same_site == SameSite::None || self.is_https() {
            header.push_str("; Secure");
        }
        header.push_str(match options.same_site {
            SameSite::Lax => "; SameSite=Lax",
            SameSite::Strict => "; SameSite=Strict",
            SameSite::None => "; SameSite=None",
        });
        // Every part was checked above: no need to check the whole again.
        self.put("set-cookie", Cow::Owned(header));
    }

    /// Whether the visitor's browser reached the site over HTTPS, as far as
    /// the app can tell: TLS ends at the proxy in front of it.
    fn is_https(&self) -> bool {
        let forwarded = self
            .forwarded("x-forwarded-proto")
            .is_some_and(|p| p.eq_ignore_ascii_case("https"));
        forwarded
            || crate::settings()
                .origin
                .as_deref()
                .is_some_and(|o| o.starts_with("https://"))
    }

    /// The action a form posted to: `?/name` → `name`, otherwise `default`.
    #[doc(hidden)]
    pub fn action(&self) -> &str {
        match self.query_string().strip_prefix('/') {
            Some(rest) => rest.split('&').next().unwrap_or(""),
            None => "default",
        }
    }
}

#[cfg(test)]
impl Cx {
    /// Builds a Cx the way the server does: bytes in the buffer, spans into
    /// it. Each of `params` is a name and its text, found in the path.
    pub(crate) fn for_test(raw: &str, params: &[(&'static str, &str)]) -> Cx {
        // Saved tables (the sessions' sign-outs) stay out of the folder.
        crate::store::memory();
        let mut cx = Cx::new("127.0.0.1:1".parse().unwrap());
        cx.wire.buf.extend_from_slice(raw.as_bytes());
        let (head, body) = raw.split_once("\r\n\r\n").unwrap();
        let mut lines = head.split("\r\n");
        let target = lines.next().unwrap().split(' ').nth(1).unwrap();
        let at = |s: &str| Span::of(raw.as_bytes(), s.as_bytes());
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        cx.method = Method::parse(head.split(' ').next().unwrap().as_bytes());
        cx.wire.path = at(path);
        cx.wire.query = at(query);
        cx.wire.body = at(body);
        for l in lines {
            let (n, v) = l.split_once(": ").unwrap();
            cx.wire.headers.push((at(n), at(v)));
        }
        let mut spans = [Span::default(); MAX_PARAMS];
        for (span, (_, value)) in spans.iter_mut().zip(params) {
            let start = path.find(value).expect("a parameter's text is in the path");
            *span = at(&path[start..start + value.len()]);
        }
        let names: Vec<&'static str> = params.iter().map(|(n, _)| *n).collect();
        cx.set_params(names.leak(), spans);
        cx
    }
}

/// `v` parsed as a `T`, or `default` when it is missing or does not parse.
fn parsed_or<T: FromStr>(v: Option<&str>, default: T) -> T {
    v.and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// A fresh request id: 8 random hex digits per process, then a counter, so
/// two are never alike within a process and, but for a 1 in 4 billion
/// chance, across processes either.
fn new_id() -> String {
    static SEED: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let seed = *SEED.get_or_init(|| u32::from_le_bytes(crate::sign::random()));
    let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    crate::hex(&(u64::from(seed) << 32 | u64::from(n)).to_be_bytes())
}

/// A name of visible ASCII but `:`, and a value with no CR, LF or NUL,
/// eight bytes at a time: nothing that would end the line or the field.
pub(crate) fn valid_header(name: &str, value: &str) -> bool {
    use crate::swar::{above, below, eq, none};
    !name.is_empty()
        && none(name.as_bytes(), |x| {
            below(x, 0x21) | above(x, 0x7e) | eq(x, b':')
        })
        && none(value.as_bytes(), |x| eq(x, b'\r') | eq(x, b'\n') | eq(x, 0))
}

/// How a cookie is kept, for [`Cx::set_cookie_with`]. The default is what
/// [`Cx::set_cookie`] does: the whole site, 400 days, hidden from scripts,
/// `SameSite=Lax`, unsigned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CookieOptions {
    /// How long the browser keeps it. `None` keeps it until the browser
    /// closes.
    pub max_age: Option<Duration>,
    /// Lets the page's scripts read it (`document.cookie`). Off by default,
    /// so a script injected into a page cannot steal it.
    pub script_readable: bool,
    pub same_site: SameSite,
    /// The paths it is sent to, `/` and everything below by default.
    pub path: &'static str,
    /// Sends it to subdomains too: `Some("example.com")` covers
    /// `app.example.com`. `None`, the default, keeps it to this host.
    pub domain: Option<&'static str>,
    /// Signed with the app's secret; read it with [`Cx::signed_cookie`].
    pub signed: bool,
}

impl Default for CookieOptions {
    fn default() -> CookieOptions {
        CookieOptions {
            max_age: Some(Duration::from_secs(400 * 24 * 60 * 60)),
            script_readable: false,
            same_site: SameSite::Lax,
            path: "/",
            domain: None,
            signed: false,
        }
    }
}

/// Which requests from other sites carry the cookie.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SameSite {
    /// Links from other sites carry it; their forms and scripts do not.
    Lax,
    /// Only requests from this site carry it, so a visitor who follows a
    /// link here from elsewhere looks signed out on that first page.
    Strict,
    /// Every request carries it, embeds on other sites included. Implies
    /// `Secure`: browsers only keep such cookies from HTTPS sites.
    None,
}

/// Percent-decodes `s`, borrowing when nothing needs decoding. Invalid
/// escapes are kept literally; invalid UTF-8 becomes U+FFFD.
pub fn decode(s: &[u8], plus_is_space: bool) -> Cow<'_, str> {
    let needs = s.iter().any(|&b| b == b'%' || (plus_is_space && b == b'+'));
    if !needs {
        return String::from_utf8_lossy(s);
    }
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'+' if plus_is_space => out.push(b' '),
            b'%' => match (
                s.get(i + 1).copied().and_then(hex_digit),
                s.get(i + 2).copied().and_then(hex_digit),
            ) {
                (Some(h), Some(l)) => {
                    out.push(h << 4 | l);
                    i += 2;
                }
                _ => out.push(b'%'),
            },
            b => out.push(b),
        }
        i += 1;
    }
    Cow::Owned(String::from_utf8_lossy(&out).into_owned())
}

/// Percent-encodes `s` into `out`: each byte `keep` refuses as `%XX`.
/// `keep` takes ASCII only.
pub(crate) fn encode(
    out: &mut impl std::fmt::Write,
    s: &str,
    keep: fn(u8) -> bool,
) -> std::fmt::Result {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    let b = s.as_bytes();
    let mut done = 0;
    for (i, &c) in b.iter().enumerate() {
        if keep(c) {
            continue;
        }
        // Everything since the last escape was kept, so is ASCII.
        out.write_str(std::str::from_utf8(&b[done..i]).unwrap_or(""))?;
        for d in [b'%', DIGITS[(c >> 4) as usize], DIGITS[(c & 15) as usize]] {
            out.write_char(d as char)?;
        }
        done = i + 1;
    }
    out.write_str(std::str::from_utf8(&b[done..]).unwrap_or(""))
}

/// What a URL carries as itself (RFC 3986's unreserved characters); the
/// rest [`encode`] escapes.
pub(crate) fn unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~')
}

/// The value of a hex digit, either case.
pub(crate) fn hex_digit(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

// ---- values kept in cookies: #[derive(Cookie)] -------------------------------

/// Writes the fields of a `#[derive(Cookie)]` type: `|` between fields, and
/// in a field `%XX` for `|`, `%` and whatever a cookie cannot hold.
#[doc(hidden)]
pub struct CookieWriter<'a, 'f> {
    f: &'a mut std::fmt::Formatter<'f>,
    first: bool,
}

impl<'a, 'f> CookieWriter<'a, 'f> {
    pub fn new(f: &'a mut std::fmt::Formatter<'f>) -> Self {
        CookieWriter { f, first: true }
    }

    pub fn field<T: std::fmt::Display + ?Sized>(&mut self, value: &T) -> std::fmt::Result {
        use std::fmt::Write;
        if !self.first {
            self.f.write_char('|')?;
        }
        self.first = false;
        write!(Escaped(self.f), "{value}")
    }
}

/// The cookie [`Cx::flash`] keeps its message in.
const FLASH: &str = "wisp-flash";

/// Text written for a cookie, escaped as a `#[derive(Cookie)]` field is.
struct Escape<'a>(&'a str);

impl std::fmt::Display for Escape<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        CookieWriter::new(f).field(self.0)
    }
}

struct Escaped<'a, 'f>(&'a mut std::fmt::Formatter<'f>);

impl std::fmt::Write for Escaped<'_, '_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        encode(self.0, s, |c| {
            c.is_ascii_graphic() && !matches!(c, b'"' | b',' | b';' | b'\\' | b'|' | b'%')
        })
    }
}

/// Reads back what `CookieWriter` wrote, one field at a time.
#[doc(hidden)]
pub struct CookieReader<'a> {
    rest: Option<&'a str>,
}

impl<'a> CookieReader<'a> {
    pub fn new(s: &'a str) -> Self {
        CookieReader { rest: Some(s) }
    }

    pub fn text(&mut self) -> Result<Cow<'a, str>, BadCookie> {
        let s = self.rest.ok_or(BadCookie)?;
        let (field, rest) = match s.split_once('|') {
            Some((field, rest)) => (field, Some(rest)),
            None => (s, None),
        };
        self.rest = rest;
        Ok(decode(field.as_bytes(), false))
    }

    pub fn field<T: std::str::FromStr>(&mut self) -> Result<T, BadCookie> {
        self.text()?.parse().map_err(|_| BadCookie)
    }

    /// Every field was read: a cookie with more is not one of these.
    pub fn end(&self) -> Result<(), BadCookie> {
        if self.rest.is_none() {
            Ok(())
        } else {
            Err(BadCookie)
        }
    }
}

/// A cookie that is not a value of the type it was read as.
#[derive(Debug, PartialEq)]
pub struct BadCookie;

impl std::fmt::Display for BadCookie {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("not a cookie of this type")
    }
}

impl std::error::Error for BadCookie {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoding() {
        assert!(matches!(decode(b"plain", true), Cow::Borrowed("plain")));
        assert_eq!(decode(b"a+b%20c%2Fd", true), "a b c/d");
        assert_eq!(decode(b"a+b", false), "a+b");
        assert_eq!(decode(b"100%", true), "100%");
        assert_eq!(decode(b"%zz%4", true), "%zz%4");
        assert_eq!(decode(b"%C3%BC", true), "ü");
        assert_eq!(decode(b"%FF", true), "\u{FFFD}");
    }

    #[test]
    fn paths_split_into_segments() {
        let mut segs = [""; MAX_SEGS];
        assert_eq!(split("/", &mut segs), Some(&[][..]));
        assert_eq!(split("/a//b", &mut segs), Some(&["a", "", "b"][..]));
        assert_eq!(split(&"/x".repeat(40), &mut segs), None);
        let mut two = [""; 2];
        assert_eq!(split("/a/b", &mut two), Some(&["a", "b"][..]));
        assert_eq!(split("/a/b/c", &mut two), None, "deeper than any arm");
        assert_eq!(split("", &mut two), None);
    }

    #[test]
    fn basic_auth_and_cors() {
        let cx = cx_for("GET / HTTP/1.1\r\nAuthorization: Basic YWRhOmx1diA6eA==\r\n\r\n");
        assert_eq!(cx.basic_auth(), Some(("ada".into(), "luv :x".into())));
        let mut out = [0; 4];
        assert_eq!(sign::unbase64("YQ", &mut out), Some(1));
        assert_eq!(out[0], b'a');
        assert_eq!(sign::unbase64("Y*==", &mut out), None);
        assert_eq!(sign::unbase64("YWJjZA", &mut out[..3]), None, "too long");
        assert_eq!(
            cx_for("GET / HTTP/1.1\r\nAuthorization: Bearer x\r\n\r\n").basic_auth(),
            None
        );

        let headers = |cx: &Cx| -> Vec<String> {
            cx.out_headers
                .iter()
                .map(|(n, v)| format!("{n}: {v}"))
                .collect()
        };
        let mut same = cx_for("GET / HTTP/1.1\r\n\r\n");
        assert!(same.cors("*").is_none() && same.out_headers.is_empty());
        let mut get = cx_for("GET / HTTP/1.1\r\nOrigin: https://a.example\r\n\r\n");
        assert!(get.cors("https://b.example, https://a.example/").is_none());
        assert_eq!(
            headers(&get),
            [
                "vary: origin",
                "access-control-allow-origin: https://a.example",
                "access-control-allow-credentials: true"
            ]
        );
        let mut other = cx_for("GET / HTTP/1.1\r\nOrigin: https://evil.example\r\n\r\n");
        assert!(other.cors("https://a.example").is_none());
        assert_eq!(headers(&other), ["vary: origin"]);
        let mut pre = cx_for(
            "OPTIONS /api HTTP/1.1\r\nOrigin: https://x.example\r\nAccess-Control-Request-Method: PUT\r\nAccess-Control-Request-Headers: content-type\r\n\r\n",
        );
        let r = pre.cors("*").expect("a preflight is answered");
        assert_eq!(r.status, 204);
        let names: Vec<&str> = r.headers.iter().map(|(n, _)| &**n).collect();
        assert_eq!(
            names,
            [
                "access-control-allow-methods",
                "access-control-max-age",
                "access-control-allow-headers"
            ]
        );
        assert_eq!(headers(&pre)[1], "access-control-allow-origin: *");
    }

    #[test]
    fn a_failed_form() {
        struct Problem(&'static str);
        let mut cx = cx_for("POST / HTTP/1.1\r\n\r\n");
        cx.fail(422, Problem("Choose a photo"));
        assert_eq!(
            (cx.status, cx.get::<Problem>().map(|p| p.0)),
            (422, Some("Choose a photo"))
        );
        assert_eq!(cx.take::<Problem>().map(|p| p.0), Some("Choose a photo"));
        assert!(cx.get::<Problem>().is_none() && cx.take::<Problem>().is_none());
    }

    #[test]
    fn flash_messages_are_read_once() {
        let mut cx = cx_for("POST / HTTP/1.1\r\n\r\n");
        cx.flash("Saved; 100% | done");
        assert_eq!(
            cx.out_headers[0].1,
            "wisp-flash=Saved%3B%20100%25%20%7C%20done; Path=/; HttpOnly; SameSite=Lax"
        );
        assert_eq!(cx.flashed().as_deref(), Some("Saved; 100% | done"));
        assert_eq!(cx.flashed(), None);
        let mut next = cx_for("GET / HTTP/1.1\r\nCookie: wisp-flash=Hi%20there\r\n\r\n");
        assert_eq!(next.flashed().as_deref(), Some("Hi there"));
        assert!(
            next.out_headers[0]
                .1
                .starts_with("wisp-flash=; Path=/; Max-Age=0;")
        );
        assert_eq!(cx_for("GET / HTTP/1.1\r\n\r\n").flashed(), None);
    }

    #[test]
    fn bearer_host_and_deleting() {
        let mut cx = cx_for(
            "GET / HTTP/1.1\r\nHost: a.test:80\r\nAuthorization: bearer  tok\r\nCookie: a=1\r\n\r\n",
        );
        assert_eq!((cx.host(), cx.bearer()), (Some("a.test:80"), Some("tok")));
        let basic = cx_for("GET / HTTP/1.1\r\nAuthorization: Basic x\r\n\r\n");
        assert_eq!(basic.bearer(), None);
        cx.delete_cookie("a");
        assert_eq!(cx.cookie("a"), None);
    }

    #[test]
    fn request_ids() {
        let cx = cx_for("GET / HTTP/1.1\r\nX-Request-Id: abc-123\r\n\r\n");
        assert_eq!(cx.id(), None);
        assert_eq!(cx.request_id(), "abc-123");
        assert_eq!(cx.id(), Some("abc-123"));
        let none = cx_for("GET / HTTP/1.1\r\n\r\n");
        let made = none.request_id();
        assert_eq!(made.len(), 16);
        assert_eq!(none.request_id(), made);
        assert_ne!(cx_for("GET / HTTP/1.1\r\n\r\n").request_id(), made);
        let long = format!(
            "GET / HTTP/1.1\r\nX-Request-Id: {}\r\n\r\n",
            "a".repeat(129)
        );
        assert_eq!(cx_for(&long).request_id().len(), 16);
        let bad = cx_for("GET / HTTP/1.1\r\nX-Request-Id: a b\r\n\r\n");
        assert_ne!(bad.request_id(), "a b");
    }

    fn cx_for(raw: &str) -> Cx {
        Cx::for_test(raw, &[])
    }

    #[test]
    fn context() {
        let raw = "POST /blog/caf%C3%A9?/like&x=1 HTTP/1.1\r\nCookie: a=1; session=\"xyz\"\r\n\
                   Content-Type: application/x-www-form-urlencoded;charset=UTF-8\r\n\r\nid=7";
        let mut cx = cx_for(raw);
        let at = raw.find("caf%").unwrap();
        let slug = Span::of(raw.as_bytes(), &raw.as_bytes()[at..at + "caf%C3%A9".len()]);
        let mut spans = [Span::default(); MAX_PARAMS];
        spans[0] = slug;
        cx.set_params(&["slug"], spans);
        assert_eq!(cx.path(), "/blog/caf%C3%A9");
        assert_eq!(cx.param("slug"), "café");
        assert_eq!(cx.cookie("session"), Some("xyz"));
        assert_eq!(cx.header("content-type").map(str::len), Some(47));
        assert_eq!(cx.form().get("id").unwrap(), "7");
        assert_eq!(cx.action(), "like");
        assert_eq!(cx.query("x").unwrap(), "1");
        assert_eq!(cx.query_or("x", 0), 1);
        assert_eq!(cx.query_or("nope", 5), 5);
        assert_eq!(
            cx.headers().map(|(n, _)| n).collect::<Vec<_>>(),
            ["Cookie", "Content-Type"]
        );
        assert_eq!(cx.client_ip(), cx.peer().ip());
    }

    #[test]
    fn values_handed_along_a_request() {
        #[derive(Debug, PartialEq)]
        struct User(u32);
        let mut cx = cx_for("GET / HTTP/1.1\r\n\r\n");
        assert_eq!(cx.get::<User>(), None);
        cx.set(User(1));
        cx.set(User(2));
        cx.set("text");
        assert_eq!(cx.get::<User>(), Some(&User(2)));
        assert_eq!(cx.get::<&str>(), Some(&"text"));
        cx.reset();
        assert_eq!(cx.get::<User>(), None);
    }

    #[test]
    fn signed_cookies() {
        let mut cx = cx_for("GET / HTTP/1.1\r\nCookie: forged=7.AAAA; plain=7\r\n\r\n");
        cx.set_signed_cookie("user", 42);
        assert_eq!(cx.signed_cookie("user"), Some("42"));
        assert_eq!(cx.signed_cookie_or("user", 0), 42);
        let (stored, mac) = cx.cookie("user").unwrap().split_once('.').unwrap();
        assert_eq!((stored, mac.len()), ("42", 43));
        assert_eq!(cx.signed_cookie("forged"), None);
        assert_eq!(cx.signed_cookie("plain"), None);

        // The same value under another name, or another value, fails.
        let copied = format!("GET / HTTP/1.1\r\nCookie: admin=42.{mac}; user=43.{mac}\r\n\r\n");
        let cx = cx_for(&copied);
        assert_eq!(
            (cx.signed_cookie("admin"), cx.signed_cookie("user")),
            (None, None)
        );
    }

    #[test]
    fn cookie_options() {
        let mut cx = cx_for("GET / HTTP/1.1\r\nX-Forwarded-Proto: https\r\n\r\n");
        let session = CookieOptions {
            max_age: None,
            script_readable: true,
            same_site: SameSite::Strict,
            ..CookieOptions::default()
        };
        cx.set_cookie_with("theme", "dark", session);
        assert_eq!(
            cx.out_headers[0].1,
            "theme=dark; Path=/; Secure; SameSite=Strict"
        );
        let scoped = CookieOptions {
            path: "/admin",
            domain: Some("example.com"),
            same_site: SameSite::None,
            ..CookieOptions::default()
        };
        let mut cx = cx_for("GET / HTTP/1.1\r\n\r\n");
        cx.set_cookie_with("a", 1, scoped);
        assert_eq!(
            cx.out_headers[0].1,
            "a=1; Path=/admin; Domain=example.com; Max-Age=34560000; HttpOnly; Secure; SameSite=None"
        );
    }

    #[test]
    fn cookies() {
        let mut cx = cx_for("GET / HTTP/1.1\r\nCookie: count=1; theme=dark\r\n\r\n");
        assert_eq!(cx.cookie_or("count", 0), 1);
        cx.set_cookie("count", 2);
        cx.set_cookie("theme", "");
        assert_eq!(cx.cookie("count"), Some("2"));
        assert_eq!(cx.cookie("theme"), None);
        assert_eq!(cx.cookie_or("theme", 7), 7);
        assert_eq!(
            cx.out_headers[0].1,
            "count=2; Path=/; Max-Age=34560000; HttpOnly; SameSite=Lax"
        );
        assert!(
            cx.out_headers[1]
                .1
                .starts_with("theme=; Path=/; Max-Age=0;")
        );
        cx.reset();
        assert_eq!(cx.cookie("count"), Some("1"));
    }

    #[test]
    fn cookie_fields_round_trip() {
        struct Two<'a>(&'a str, u32);
        impl std::fmt::Display for Two<'_> {
            fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                let mut w = CookieWriter::new(f);
                w.field(self.0)?;
                w.field(&self.1)
            }
        }
        let text = Two("a|b 100% ü;", 7).to_string();
        assert_eq!(text, "a%7Cb%20100%25%20%C3%BC%3B|7");
        let mut r = CookieReader::new(&text);
        assert_eq!(r.text().unwrap(), "a|b 100% ü;");
        assert_eq!(r.field::<u32>(), Ok(7));
        assert_eq!(r.end(), Ok(()));
        assert_eq!(r.field::<u32>(), Err(BadCookie));
        assert_eq!(CookieReader::new("x").field::<u32>(), Err(BadCookie));
        let mut r = CookieReader::new("1|2|3");
        assert_eq!((r.field::<u8>(), r.field::<u8>()), (Ok(1), Ok(2)));
        assert_eq!(r.end(), Err(BadCookie));
    }

    #[test]
    fn cookies_from_anyone_and_back() {
        use crate::fuzz::{Rng, mutate};
        let mut rng = Rng::new(20);
        for _ in 0..3000 {
            // Whatever a browser, or anyone, sends as `Cookie`.
            let mut header = rng.upto(60, b"ab=; \"\t,%|\x80");
            mutate(&mut rng, &mut header);
            let header = String::from_utf8_lossy(&header).replace(['\r', '\n'], "");
            let cx = cx_for(&format!("GET / HTTP/1.1\r\nCookie: {header}\r\n\r\n"));
            for name in ["a", "b", "", "wisp-flash"] {
                let _ = (
                    cx.cookie(name),
                    cx.signed_cookie(name),
                    cx.cookie_or(name, 0u8),
                );
            }
            let _ = CookieReader::new(&header).text();
            let _ = sign::unbase64(&header, &mut [0; 64]);

            // Any text survives a flash, and any two fields a cookie type.
            let text = rng.text(30);
            let mut cx = cx_for("POST / HTTP/1.1\r\n\r\n");
            cx.flash(&text);
            let flashed = (!text.is_empty()).then_some(text.as_str());
            assert_eq!(cx.flashed().as_deref(), flashed, "an empty one is none");
            struct Two(String, i64);
            impl std::fmt::Display for Two {
                fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    let mut w = CookieWriter::new(f);
                    w.field(&self.0)?;
                    w.field(&self.1)
                }
            }
            let n = rng.next() as i64;
            cx.set_signed_cookie("two", Two(text.clone(), n));
            let mut r = CookieReader::new(cx.signed_cookie("two").unwrap());
            assert_eq!(
                (r.text().unwrap(), r.field::<i64>()),
                (text.as_str().into(), Ok(n))
            );
            assert_eq!(r.end(), Ok(()));
        }
    }

    #[test]
    fn queries_and_params_decode_anything() {
        use crate::fuzz::Rng;
        let mut rng = Rng::new(21);
        for _ in 0..3000 {
            let raw = rng.upto(40, b"a=&%+2Fe9\xc3\xa9\xff");
            let decoded = decode(&raw, true);
            assert!(decoded.len() <= raw.len() * 3, "U+FFFD is 3 bytes at most");
            let text = String::from_utf8_lossy(&raw).replace([' ', '\r', '\n', '#'], "");
            let cx = cx_for(&format!("GET /p/{text}?{text} HTTP/1.1\r\n\r\n"));
            let _ = (
                cx.query("a"),
                cx.query_or("e", 0),
                cx.query_all("a").count(),
            );
            let _ = cx.action();
        }
    }

    #[test]
    #[should_panic(expected = "invalid cookie value")]
    fn cookie_value_is_checked() {
        cx_for("GET / HTTP/1.1\r\n\r\n").set_cookie("a", "x;y");
    }
}
