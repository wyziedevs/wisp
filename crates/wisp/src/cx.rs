//! The request context passed to `load`, actions and endpoints.
//!
//! There is one `Cx` per connection. It owns the connection's read buffer
//! and describes the current request as spans into it, so nothing is copied
//! and, once warm, nothing is allocated. Owning the buffer (rather than
//! borrowing it) keeps `Cx` free of lifetimes: handlers take `&mut Cx`.

use crate::{Error, Result};
use std::borrow::Cow;
use std::net::SocketAddr;

pub const MAX_PARAMS: usize = 8;

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
    pub(crate) fn parse(s: &str) -> Method {
        match s {
            "GET" => Method::Get,
            "HEAD" => Method::Head,
            "POST" => Method::Post,
            "PUT" => Method::Put,
            "PATCH" => Method::Patch,
            "DELETE" => Method::Delete,
            "OPTIONS" => Method::Options,
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
        Span { start: start as u32, len: part.len() as u32 }
    }

    pub fn range(self) -> std::ops::Range<usize> {
        self.start as usize..(self.start + self.len) as usize
    }
}

pub struct Cx {
    pub method: Method,
    /// The connection's read buffer: this request and any pipelined after it.
    pub(crate) buf: Vec<u8>,
    pub(crate) path: Span,
    pub(crate) query: Span,
    pub(crate) body: Span,
    pub(crate) headers: Vec<(Span, Span)>,
    peer: SocketAddr,
    names: &'static [&'static str],
    params: [Span; MAX_PARAMS],
    /// Percent-decoded copies, only for params that needed decoding.
    decoded: [Option<String>; MAX_PARAMS],
    pub(crate) status: u16,
    pub(crate) out_headers: Vec<(Cow<'static, str>, String)>,
    /// Cookies set by this request, which `cookie` reads before the request's
    /// own: a page's `load` sees what its action just stored.
    set_cookies: Vec<(String, String)>,
}

impl Cx {
    pub(crate) fn new(peer: SocketAddr) -> Cx {
        Cx {
            method: Method::Get,
            buf: Vec::with_capacity(8 * 1024),
            path: Span::default(),
            query: Span::default(),
            body: Span::default(),
            headers: Vec::with_capacity(16),
            peer,
            names: &[],
            params: [Span::default(); MAX_PARAMS],
            decoded: [const { None }; MAX_PARAMS],
            status: 200,
            out_headers: Vec::new(),
            set_cookies: Vec::new(),
        }
    }

    /// Clears per-request state; request spans are set by the parser.
    pub(crate) fn reset(&mut self) {
        self.names = &[];
        self.decoded.iter_mut().for_each(|d| *d = None);
        self.status = 200;
        self.out_headers.clear();
        self.set_cookies.clear();
    }

    pub(crate) fn set_params(&mut self, names: &'static [&'static str], spans: [Span; MAX_PARAMS]) {
        debug_assert!(names.len() <= MAX_PARAMS);
        self.names = names;
        self.params = spans;
        for (span, decoded) in spans.iter().zip(&mut self.decoded).take(names.len()) {
            if let Cow::Owned(s) = decode(&self.buf[span.range()], false) {
                *decoded = Some(s);
            }
        }
    }

    fn str(&self, span: Span) -> &str {
        // Request lines and header names are validated ASCII by httparse.
        std::str::from_utf8(&self.buf[span.range()]).unwrap_or("")
    }

    /// The URL path, as sent (not percent-decoded).
    pub fn path(&self) -> &str {
        self.str(self.path)
    }

    /// The raw query string, without the `?`.
    pub fn query_string(&self) -> &str {
        self.str(self.query)
    }

    /// A route parameter such as `slug` in `blog/[slug]`, percent-decoded.
    /// Optional parameters that are absent are `""`.
    ///
    /// Panics if the route has no such parameter: that is a typo in code,
    /// not bad input.
    pub fn param(&self, name: &str) -> &str {
        match self.names.iter().position(|n| *n == name) {
            Some(i) => self.decoded[i].as_deref().unwrap_or_else(|| self.str(self.params[i])),
            None => panic!("route has no parameter `{name}` (it has {:?})", self.names),
        }
    }

    /// First query parameter named `name`, decoded.
    pub fn query(&self, name: &str) -> Option<Cow<'_, str>> {
        pairs(&self.buf[self.query.range()]).find(|(k, _)| k == name).map(|(_, v)| v)
    }

    /// The urlencoded request body. Empty for any other content type.
    pub fn form(&self) -> Form<'_> {
        let urlencoded = self.header("content-type").is_some_and(|ct| {
            ct.as_bytes().get(..33).is_some_and(|p| p.eq_ignore_ascii_case(b"application/x-www-form-urlencoded"))
        });
        Form { body: if urlencoded { self.body() } else { &[] } }
    }

    pub fn body(&self) -> &[u8] {
        &self.buf[self.body.range()]
    }

    /// Request header by case-insensitive name. Non-UTF-8 values are `None`.
    pub fn header(&self, name: &str) -> Option<&str> {
        let (_, v) = self.headers.iter().find(|(n, _)| self.buf[n.range()].eq_ignore_ascii_case(name.as_bytes()))?;
        std::str::from_utf8(&self.buf[v.range()]).ok()
    }

    /// A cookie's value: the one this request set with `set_cookie`, else
    /// the one the browser sent. A deleted cookie is `None`.
    pub fn cookie(&self, name: &str) -> Option<&str> {
        if let Some((_, value)) = self.set_cookies.iter().rev().find(|(n, _)| n == name) {
            return (!value.is_empty()).then_some(value.as_str());
        }
        let all = self.header("cookie")?;
        all.split(';').find_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            (k.trim() == name).then(|| v.trim().trim_matches('"'))
        })
    }

    pub fn peer(&self) -> SocketAddr {
        self.peer
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
        assert!(valid_header(&name, &value), "invalid header {name:?}: {value:?}");
        self.out_headers.push((name, value));
    }

    /// Sets a cookie for the whole site, kept for 400 days (the most browsers
    /// allow) and hidden from page scripts. An empty value deletes it.
    ///
    /// Panics on a character a cookie cannot hold (space, `"`, `,`, `;`,
    /// `\`, control or non-ASCII); encode such values first.
    pub fn set_cookie(&mut self, name: &str, value: &str) {
        let token = |s: &str, bad: &[u8]| s.bytes().all(|b| b.is_ascii_graphic() && !bad.contains(&b));
        assert!(!name.is_empty() && token(name, b"()<>@,;:\\\"/[]?={}"), "invalid cookie name {name:?}");
        assert!(token(value, b"\",;\\"), "invalid cookie value {value:?}");
        let age = if value.is_empty() { 0 } else { 400 * 24 * 60 * 60 };
        self.set_header("set-cookie", format!("{name}={value}; Path=/; Max-Age={age}; HttpOnly; SameSite=Lax"));
        self.set_cookies.push((name.to_owned(), value.to_owned()));
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

pub(crate) fn valid_header(name: &str, value: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|b| b.is_ascii_graphic() && b != b':')
        && !value.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0)
}

/// A urlencoded form body. `Copy`, borrows the request, decodes on lookup.
#[derive(Clone, Copy)]
pub struct Form<'a> {
    body: &'a [u8],
}

impl<'a> Form<'a> {
    pub fn get(&self, name: &str) -> Option<Cow<'a, str>> {
        pairs(self.body).find(|(k, _)| k == name).map(|(_, v)| v)
    }

    /// Like `get`, but a missing field is a 400 error.
    pub fn required(&self, name: &str) -> Result<Cow<'a, str>> {
        self.get(name).ok_or_else(|| Error::new(400, format!("missing form field `{name}`")))
    }

    /// Every value of a repeated field, like checkboxes with the same name.
    pub fn all<'n>(&self, name: &'n str) -> impl Iterator<Item = Cow<'a, str>> + 'n
    where
        'a: 'n,
    {
        pairs(self.body).filter(move |(k, _)| k == name).map(|(_, v)| v)
    }

    pub fn iter(&self) -> impl Iterator<Item = (Cow<'a, str>, Cow<'a, str>)> {
        pairs(self.body)
    }
}

/// `a=1&b=x%20y` → decoded pairs. `+` is a space, as in HTML forms.
fn pairs(s: &[u8]) -> impl Iterator<Item = (Cow<'_, str>, Cow<'_, str>)> {
    s.split(|&b| b == b'&').filter(|kv| !kv.is_empty()).map(|kv| {
        let (k, v) = match kv.iter().position(|&b| b == b'=') {
            Some(i) => (&kv[..i], &kv[i + 1..]),
            None => (kv, &[][..]),
        };
        (decode(k, true), decode(v, true))
    })
}

/// Percent-decodes `s`, borrowing when nothing needs decoding. Invalid
/// escapes are kept literally; invalid UTF-8 becomes U+FFFD.
pub(crate) fn decode(s: &[u8], plus_is_space: bool) -> Cow<'_, str> {
    let needs = s.iter().any(|&b| b == b'%' || (plus_is_space && b == b'+'));
    if !needs {
        return String::from_utf8_lossy(s);
    }
    let hex = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    };
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'+' if plus_is_space => out.push(b' '),
            b'%' => match (s.get(i + 1).copied().and_then(hex), s.get(i + 2).copied().and_then(hex)) {
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
    fn forms() {
        let f = Form { body: b"title=Hello+world&tag=a&tag=b&empty=&flag" };
        assert_eq!(f.get("title").unwrap(), "Hello world");
        assert_eq!(f.get("empty").unwrap(), "");
        assert_eq!(f.get("flag").unwrap(), "");
        assert!(f.get("nope").is_none());
        assert_eq!(f.all("tag").collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(f.required("nope").unwrap_err().status(), 400);
    }

    /// Builds a Cx the way the server does: bytes in the buffer, spans into it.
    fn cx_for(raw: &str) -> Cx {
        let mut cx = Cx::new("127.0.0.1:1".parse().unwrap());
        cx.buf.extend_from_slice(raw.as_bytes());
        let (head, body) = raw.split_once("\r\n\r\n").unwrap();
        let mut lines = head.split("\r\n");
        let target = lines.next().unwrap().split(' ').nth(1).unwrap();
        let at = |s: &str| Span::of(raw.as_bytes(), s.as_bytes());
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        cx.path = at(path);
        cx.query = at(query);
        cx.body = at(body);
        for l in lines {
            let (n, v) = l.split_once(": ").unwrap();
            cx.headers.push((at(n), at(v)));
        }
        cx
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
    }

    #[test]
    fn cookies() {
        let mut cx = cx_for("GET / HTTP/1.1\r\nCookie: count=1; theme=dark\r\n\r\n");
        cx.set_cookie("count", "2");
        cx.set_cookie("theme", "");
        assert_eq!(cx.cookie("count"), Some("2"));
        assert_eq!(cx.cookie("theme"), None);
        assert_eq!(cx.out_headers[0].1, "count=2; Path=/; Max-Age=34560000; HttpOnly; SameSite=Lax");
        assert!(cx.out_headers[1].1.starts_with("theme=; Path=/; Max-Age=0;"));
        cx.reset();
        assert_eq!(cx.cookie("count"), Some("1"));
    }

    #[test]
    #[should_panic(expected = "invalid cookie value")]
    fn cookie_value_is_checked() {
        cx_for("GET / HTTP/1.1\r\n\r\n").set_cookie("a", "x;y");
    }
}
