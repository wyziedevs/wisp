//! Outbound HTTP from the server: `wisp::fetch(request).await?`, the same
//! `Request` and `Reply` as the edge build's `wisp::edge::fetch`, so code
//! that calls an API runs in both.
//!
//! ```ignore
//! let mut req = wisp::Request::new("POST", "https://api.example.com/rows");
//! req.header("authorization", &format!("Bearer {key}"));
//! req.body = json.into_bytes();
//! let reply = wisp::fetch(req).await?;   // reply.status, reply.text()
//! ```
//!
//! `http://` is plain TCP; `https://` needs the `tls` feature (rustls with
//! the web's root certificates), which is off by default, so an app that
//! calls nothing carries none of it. One request on one connection, no
//! redirects followed (a 3xx is the reply), no cookies, no reuse. At most
//! 30 s in all and 16 MB of reply. A request that got no answer is a 502
//! (a 504 when it timed out); the error names the host, never the path or
//! query, which may hold a key. Whatever URL it is given it calls, so a
//! URL a visitor chose is for the app to check first.

use crate::{Error, Reply, Request, Result};
use std::borrow::Cow;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

/// The longest a call takes, connecting, sending and reading.
const TIMEOUT: Duration = Duration::from_secs(30);

/// The most of a reply read, head and body.
const MAX: usize = 16 << 20;

static HOOK: std::sync::OnceLock<fn(&mut Request)> = std::sync::OnceLock::new();

/// `wisp::on_fetch(|req| req.header("x-key", KEY))` in `init`: every
/// [`fetch`] passes through it first, to add a header, change a host or
/// refuse. Once; without it a call looks at nothing.
pub fn on_fetch(hook: fn(&mut Request)) {
    let _ = HOOK.set(hook);
}

/// Makes an HTTP request: to an API, a webhook, an OAuth provider.
/// `req.target` is the whole URL.
pub async fn fetch(mut req: Request) -> Result<Reply> {
    if let Some(hook) = HOOK.get() {
        hook(&mut req);
    }
    let url = Url::parse(&req.target)
        .ok_or_else(|| Error::new(500, "fetch needs an http:// or https:// URL"))?;
    let fail = |why: &str| Error::new(502, format!("fetch to {} failed: {why}", url.host));
    let wire = url
        .request(&req)
        .ok_or_else(|| Error::new(500, "fetch was given a method or header that is not valid"))?;
    let head = req.method == "HEAD";
    let call = async {
        let tcp = TcpStream::connect((url.host.as_str(), url.port))
            .await
            .map_err(|_| "could not connect")?;
        let _ = tcp.set_nodelay(true);
        if url.https {
            talk(secure(tcp, &url.host).await?, &wire, head).await
        } else {
            talk(tcp, &wire, head).await
        }
    };
    match tokio::time::timeout(TIMEOUT, call).await {
        Ok(Ok(reply)) => Ok(reply),
        Ok(Err(why)) => Err(fail(why)),
        Err(_) => Err(Error::new(504, format!("fetch to {} timed out", url.host))),
    }
}

#[cfg(feature = "tls")]
async fn secure(
    tcp: TcpStream,
    host: &str,
) -> std::result::Result<tokio_rustls::client::TlsStream<TcpStream>, &'static str> {
    use std::sync::{Arc, OnceLock};
    static CONFIG: OnceLock<Arc<tokio_rustls::rustls::ClientConfig>> = OnceLock::new();
    let config = CONFIG.get_or_init(|| {
        let roots = tokio_rustls::rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        Arc::new(
            tokio_rustls::rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        )
    });
    let name = tokio_rustls::rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|_| "not a host name")?;
    tokio_rustls::TlsConnector::from(config.clone())
        .connect(name, tcp)
        .await
        .map_err(|_| "the TLS handshake failed")
}

#[cfg(not(feature = "tls"))]
async fn secure(_: TcpStream, _: &str) -> std::result::Result<TcpStream, &'static str> {
    Err("https needs wisp's `tls` feature")
}

/// A URL split for a request.
struct Url {
    https: bool,
    host: String,
    port: u16,
    /// Path and query, with no fragment.
    path: String,
}

impl Url {
    fn parse(url: &str) -> Option<Url> {
        let (https, rest) = match url.split_once("://")? {
            (s, rest) if s.eq_ignore_ascii_case("https") => (true, rest),
            (s, rest) if s.eq_ignore_ascii_case("http") => (false, rest),
            _ => return None,
        };
        let rest = rest.split('#').next()?;
        let at = rest.find(['/', '?']).unwrap_or(rest.len());
        let (authority, path) = rest.split_at(at);
        // No `user:pass@`: it would end up in a log, and means little.
        if authority.contains('@') || !authority.bytes().all(|b| b.is_ascii_graphic()) {
            return None;
        }
        let (host, port) = match authority.strip_prefix('[') {
            Some(v6) => {
                let (host, after) = v6.split_once(']')?;
                (host, after.strip_prefix(':'))
            }
            None => match authority.rsplit_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (authority, None),
            },
        };
        let port = match port {
            Some(p) => p.parse().ok().filter(|&p| p != 0)?,
            None if https => 443,
            None => 80,
        };
        let path = match path {
            "" => "/".to_string(),
            p if p.starts_with('?') => format!("/{p}"),
            p => p.to_string(),
        };
        (!host.is_empty() && path.bytes().all(|b| b.is_ascii_graphic())).then(|| Url {
            https,
            host: host.to_string(),
            port,
            path,
        })
    }

    /// The request as HTTP/1.1 bytes, or `None` for a method or header
    /// that could split the request in two.
    fn request(&self, req: &Request) -> Option<Vec<u8>> {
        let token = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphabetic());
        if !token(&req.method) {
            return None;
        }
        let mut w = format!("{} {} HTTP/1.1\r\n", req.method, self.path);
        let host = match (self.https, self.port) {
            (true, 443) | (false, 80) if self.host.contains(':') => format!("[{}]", self.host),
            (true, 443) | (false, 80) => self.host.clone(),
            (_, port) if self.host.contains(':') => format!("[{}]:{port}", self.host),
            (_, port) => format!("{}:{port}", self.host),
        };
        w.push_str(&format!("host: {host}\r\nconnection: close\r\n"));
        let mut agent = false;
        for (n, v) in &req.headers {
            if !crate::cx::valid_header(n, v) {
                return None;
            }
            // Framing is Wisp's: an app's own would contradict it.
            let n = n.to_ascii_lowercase();
            if matches!(
                n.as_str(),
                "host" | "connection" | "content-length" | "transfer-encoding"
            ) {
                continue;
            }
            agent |= n == "user-agent";
            w.push_str(&format!("{n}: {v}\r\n"));
        }
        if !agent {
            w.push_str("user-agent: wisp\r\n");
        }
        if !req.body.is_empty() || matches!(req.method.as_str(), "POST" | "PUT" | "PATCH") {
            w.push_str(&format!("content-length: {}\r\n", req.body.len()));
        }
        w.push_str("\r\n");
        let mut bytes = w.into_bytes();
        bytes.extend_from_slice(&req.body);
        Some(bytes)
    }
}

/// Sends `wire` and reads the reply, as far as it goes.
async fn talk<S: AsyncRead + AsyncWrite + Unpin>(
    mut s: S,
    wire: &[u8],
    head: bool,
) -> std::result::Result<Reply, &'static str> {
    s.write_all(wire).await.map_err(|_| "could not send")?;
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; 16 << 10];
    loop {
        let n = s
            .read(&mut chunk)
            .await
            .map_err(|_| "the connection broke")?;
        buf.extend_from_slice(&chunk[..n]);
        if let Some(reply) = parse(&buf, n == 0, head)? {
            return Ok(reply);
        }
        if buf.len() > MAX {
            return Err("the reply is too large");
        }
    }
}

/// The reply in `buf`: `None` while more is to come, an error for one that
/// is wrong or ends early. `eof` is that the server closed.
fn parse(buf: &[u8], eof: bool, head: bool) -> std::result::Result<Option<Reply>, &'static str> {
    let mut headers = [httparse::EMPTY_HEADER; 64];
    let mut res = httparse::Response::new(&mut headers);
    let used = match res.parse(buf) {
        Ok(httparse::Status::Complete(n)) => n,
        Ok(httparse::Status::Partial) if eof => return Err("the connection closed early"),
        Ok(httparse::Status::Partial) => return Ok(None),
        Err(_) => return Err("the reply is not HTTP"),
    };
    let status = res.code.ok_or("the reply is not HTTP")?;
    let find = |name: &'static str| {
        res.headers
            .iter()
            .filter(move |h| h.name.eq_ignore_ascii_case(name))
    };
    let chunked = find("transfer-encoding").any(|h| {
        h.value
            .split(|&b| b == b',')
            .any(|t| t.trim_ascii().eq_ignore_ascii_case(b"chunked"))
    });
    let mut lengths = find("content-length").map(|h| {
        let v = std::str::from_utf8(h.value).ok()?.trim();
        v.bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| v.parse::<usize>().ok())?
    });
    // Two lengths that differ, or one that is not a number: a reply to
    // refuse, as a proxy would.
    let length = match lengths.next() {
        None => None,
        Some(l) => {
            let l = l.ok_or("the reply's length is wrong")?;
            if lengths.any(|o| o != Some(l)) {
                return Err("the reply's length is wrong");
            }
            Some(l)
        }
    };
    let rest = &buf[used..];
    let body = if head || status < 200 || status == 204 || status == 304 {
        Vec::new()
    } else if chunked {
        // A body ends with its empty line: until it does, reading it all
        // again for every chunk that arrives would cost the square of its size.
        if !eof && !rest.ends_with(b"\r\n\r\n") {
            return Ok(None);
        }
        match unchunk(rest)? {
            Some(body) => body,
            None if eof => return Err("the connection closed early"),
            None => return Ok(None),
        }
    } else if let Some(n) = length {
        if n > MAX {
            return Err("the reply is too large");
        }
        if rest.len() < n {
            if eof {
                return Err("the connection closed early");
            }
            return Ok(None);
        }
        rest[..n].to_vec()
    } else if eof {
        rest.to_vec()
    } else {
        return Ok(None);
    };
    let headers = res
        .headers
        .iter()
        .filter_map(|h| {
            let value = std::str::from_utf8(h.value).ok()?;
            Some((
                Cow::Owned(h.name.to_ascii_lowercase()),
                Cow::Owned(value.trim().to_string()),
            ))
        })
        .collect();
    Ok(Some(Reply {
        status,
        headers,
        body: crate::Body::Bytes(body),
    }))
}

/// A chunked body, whole; `None` while it is not.
fn unchunk(mut rest: &[u8]) -> std::result::Result<Option<Vec<u8>>, &'static str> {
    let mut body = Vec::new();
    loop {
        let Some(eol) = rest.windows(2).position(|w| w == b"\r\n") else {
            return if rest.len() > 32 {
                Err("a chunk is not valid")
            } else {
                Ok(None)
            };
        };
        let size = std::str::from_utf8(&rest[..eol]).map_err(|_| "a chunk is not valid")?;
        let size = size.split(';').next().unwrap_or("").trim();
        if size.is_empty() || size.len() > 8 || !size.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("a chunk is not valid");
        }
        let size = usize::from_str_radix(size, 16).map_err(|_| "a chunk is not valid")?;
        rest = &rest[eol + 2..];
        if size == 0 {
            // Trailers, then the empty line.
            let end = rest.windows(4).position(|w| w == b"\r\n\r\n");
            return Ok((rest.starts_with(b"\r\n") || end.is_some()).then_some(body));
        }
        if body.len() + size > MAX {
            return Err("the reply is too large");
        }
        if rest.len() < size + 2 {
            return Ok(None);
        }
        if &rest[size..size + 2] != b"\r\n" {
            return Err("a chunk is not valid");
        }
        body.extend_from_slice(&rest[..size]);
        rest = &rest[size + 2..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// A server that answers one request with `answer`, and sends back
    /// what it was sent.
    async fn serve(answer: &'static [u8]) -> (u16, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = s.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
            }
            // The body, if the head says there is one.
            let text = String::from_utf8_lossy(&got).to_string();
            let want = text
                .to_ascii_lowercase()
                .split("content-length: ")
                .nth(1)
                .and_then(|r| r.split("\r\n").next()?.parse::<usize>().ok())
                .unwrap_or(0);
            let have = got.len() - got.windows(4).position(|w| w == b"\r\n\r\n").unwrap() - 4;
            let mut left = want.saturating_sub(have);
            while left > 0 {
                let n = s.read(&mut buf).await.unwrap();
                got.extend_from_slice(&buf[..n]);
                left = left.saturating_sub(n);
            }
            s.write_all(answer).await.unwrap();
            let _ = s.shutdown().await;
            String::from_utf8_lossy(&got).to_string()
        });
        (port, task)
    }

    fn run<T>(f: impl Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f)
    }

    #[test]
    fn urls() {
        let u = Url::parse("http://example.com:8080/a/b?x=1#frag").unwrap();
        assert_eq!(
            (u.https, &*u.host, u.port, &*u.path),
            (false, "example.com", 8080, "/a/b?x=1")
        );
        let u = Url::parse("HTTPS://api.example.com?q=1").unwrap();
        assert_eq!((u.https, u.port, &*u.path), (true, 443, "/?q=1"));
        let u = Url::parse("http://[::1]:9/x").unwrap();
        assert_eq!((&*u.host, u.port), ("::1", 9));
        for bad in [
            "",
            "example.com",
            "ftp://example.com/",
            "http://",
            "http://user:pw@example.com/",
            "http://example.com:0/",
            "http://example.com:99999/",
            "http://exa mple.com/",
            "http://example.com/a b",
            "http://example.com/\r\nx: y",
        ] {
            assert!(Url::parse(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn calls_a_server() {
        run(async {
            let (port, got) = serve(b"HTTP/1.1 201 Created\r\nContent-Type: text/plain\r\nContent-Length: 5\r\n\r\nhello").await;
            let mut req = Request::new("POST", &format!("http://127.0.0.1:{port}/rows?k=1"));
            req.header("Authorization", "Bearer s3cret");
            req.body = b"{}".to_vec();
            let reply = fetch(req).await.unwrap();
            assert_eq!((reply.status, reply.text()), (201, "hello"));
            assert_eq!(reply.header("content-type"), Some("text/plain"));
            let sent = got.await.unwrap();
            assert!(sent.starts_with("POST /rows?k=1 HTTP/1.1\r\nhost: 127.0.0.1:"));
            assert!(sent.contains("authorization: Bearer s3cret\r\n"));
            assert!(
                sent.contains("connection: close\r\n") && sent.contains("content-length: 2\r\n")
            );
            assert!(sent.ends_with("\r\n\r\n{}"));
        });
    }

    #[test]
    fn reads_chunked_and_until_close() {
        run(async {
            let (port, _) = serve(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6;x=y\r\n world\r\n0\r\n\r\n").await;
            let reply = fetch(Request::new("GET", &format!("http://127.0.0.1:{port}")))
                .await
                .unwrap();
            assert_eq!(reply.text(), "hello world");
            let (port, _) = serve(b"HTTP/1.0 200 OK\r\n\r\nuntil the end").await;
            let reply = fetch(Request::new("GET", &format!("http://127.0.0.1:{port}/")))
                .await
                .unwrap();
            assert_eq!(reply.text(), "until the end");
            let (port, _) =
                serve(b"HTTP/1.1 302 Found\r\nLocation: /elsewhere\r\nContent-Length: 0\r\n\r\n")
                    .await;
            let reply = fetch(Request::new("GET", &format!("http://127.0.0.1:{port}/")))
                .await
                .unwrap();
            assert_eq!(
                (reply.status, reply.header("location")),
                (302, Some("/elsewhere"))
            );
        });
    }

    #[test]
    fn an_ipv6_host_is_bracketed_on_the_default_port() {
        let host = |url: &str| {
            let url = Url::parse(url).unwrap();
            let sent = url.request(&Request::new("GET", "")).unwrap();
            let sent = String::from_utf8(sent).unwrap();
            sent.lines()
                .find(|l| l.starts_with("host:"))
                .unwrap()
                .to_string()
        };
        assert_eq!(host("https://[::1]/"), "host: [::1]");
        assert_eq!(host("http://[::1]:8080/"), "host: [::1]:8080");
        assert_eq!(host("http://example.com/"), "host: example.com");
    }

    #[test]
    fn refuses_what_is_wrong_and_names_only_the_host() {
        run(async {
            // A closed port, a short body, junk, and a lie about the length.
            let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = closed.local_addr().unwrap().port();
            drop(closed);
            let e = fetch(Request::new(
                "GET",
                &format!("http://127.0.0.1:{port}/p?key=SECRET"),
            ))
            .await
            .err()
            .unwrap();
            assert_eq!(e.status(), 502);
            assert!(
                e.message().contains("127.0.0.1") && !e.message().contains("SECRET"),
                "{}",
                e.message()
            );
            for answer in [
                &b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nshort"[..],
                b"garbage\r\n\r\n",
                b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nab",
                b"HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n",
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n",
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello",
                b"",
            ] {
                let (port, _) = serve(answer).await;
                let r = fetch(Request::new("GET", &format!("http://127.0.0.1:{port}/"))).await;
                assert_eq!(r.err().map(|e| e.status()), Some(502), "{answer:?}");
            }
            let e = fetch(Request::new("GET", "https://example.com/"))
                .await
                .err()
                .unwrap();
            assert_eq!(e.status(), 502);
            assert_eq!(
                fetch(Request::new("GET", "nope"))
                    .await
                    .err()
                    .unwrap()
                    .status(),
                500
            );
        });
    }

    #[test]
    fn a_reply_too_large_is_refused() {
        run(async {
            let (mut server, client) = tokio::io::duplex(1 << 16);
            let flood = tokio::spawn(async move {
                // No length, never closing: the reader must stop by itself.
                let _ = server.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await;
                let chunk = vec![b'x'; 1 << 15];
                while server.write_all(&chunk).await.is_ok() {}
            });
            let r = talk(client, b"GET / HTTP/1.1\r\n\r\n", false).await;
            assert_eq!(r.err(), Some("the reply is too large"));
            flood.abort();
        });
        let sized = b"HTTP/1.1 200 OK\r\nContent-Length: 99999999999\r\n\r\nx";
        assert!(parse(sized, false, false).is_err());
    }
}
