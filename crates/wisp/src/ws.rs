//! WebSockets (RFC 6455) on the built-in server: the handshake, with its
//! own SHA-1, and the frame codec. A `+server.rs` answers with
//! [`Response::websocket`]; other hosts (tower, the edge build, the test
//! client) answer that with a 501.
//!
//! What the codec does: unmasks what the client sends (which must be
//! masked), puts fragmented messages back together, answers pings, echoes
//! a close, checks text is UTF-8, and refuses a message larger than the
//! route's body limit. No extensions (compression) are offered.
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use crate::{Cx, Error, Gone, Method, Response, Result};
use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;

/// A message from or to the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Text(String),
    Binary(Vec<u8>),
}

impl Message {
    /// The text of a text message; empty for a binary one.
    pub fn text(&self) -> &str {
        match self {
            Message::Text(t) => t,
            Message::Binary(_) => "",
        }
    }

    /// The bytes of either kind.
    pub fn bytes(&self) -> &[u8] {
        match self {
            Message::Text(t) => t.as_bytes(),
            Message::Binary(b) => b,
        }
    }
}

impl From<String> for Message {
    fn from(t: String) -> Message {
        Message::Text(t)
    }
}

impl From<&str> for Message {
    fn from(t: &str) -> Message {
        Message::Text(t.to_string())
    }
}

impl From<Vec<u8>> for Message {
    fn from(b: Vec<u8>) -> Message {
        Message::Binary(b)
    }
}

impl From<&[u8]> for Message {
    fn from(b: &[u8]) -> Message {
        Message::Binary(b.to_vec())
    }
}

type Handler =
    Box<dyn FnOnce(WebSocket) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send>;

/// What a [`Response::websocket`] runs once the connection is upgraded.
pub struct Upgrade(pub(crate) Handler);

impl Response {
    /// Upgrades the request to a WebSocket and runs `handler` with it; the
    /// connection closes when it returns. A page connects with
    /// `new WebSocket(url)`.
    ///
    /// ```ignore
    /// fn get() -> Response {
    ///     Response::websocket(|ws| async move {
    ///         while let Some(msg) = ws.recv().await {
    ///             ws.send(msg).await?;
    ///         }
    ///         Ok(())
    ///     })
    /// }
    /// ```
    ///
    /// A request that is not an upgrade gets a 426, and one from a page on
    /// another site a 403, as a cross-site form post would. Only the
    /// built-in server upgrades: tower and edge hosts answer 501.
    pub fn websocket<F, Fut>(handler: F) -> Response
    where
        F: FnOnce(WebSocket) -> Fut + Send + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        let mut res = Response::new("", Vec::new());
        res.status = 101;
        res.headers
            .push((Cow::Borrowed("upgrade"), "websocket".into()));
        res.headers
            .push((Cow::Borrowed("connection"), "upgrade".into()));
        res.upgrade = Some(Upgrade(Box::new(move |ws| Box::pin(handler(ws)))));
        res
    }
}

/// Checks a request to upgrade and returns its `sec-websocket-accept`.
pub(crate) fn handshake(cx: &Cx) -> Result<String> {
    let has = |name: &str, token: &str| {
        cx.headers().any(|(n, v)| {
            n.eq_ignore_ascii_case(name)
                && v.split(',').any(|t| t.trim().eq_ignore_ascii_case(token))
        })
    };
    let refuse = |status: u16, message: &'static str, header: (&'static str, &str)| Error {
        status,
        message: Cow::Borrowed(message),
        header: Some(Box::new((header.0, header.1.to_string()))),
        source: None,
        fields: Vec::new(),
        code: None,
    };
    if cx.method != Method::Get
        || !cx.http11
        || !has("upgrade", "websocket")
        || !has("connection", "upgrade")
    {
        return Err(refuse(
            426,
            "This address takes WebSocket connections",
            ("upgrade", "websocket"),
        ));
    }
    if cx.header("sec-websocket-version").map(str::trim) != Some("13") {
        return Err(refuse(
            426,
            "Only WebSocket version 13 is supported",
            ("sec-websocket-version", "13"),
        ));
    }
    let key = cx.header("sec-websocket-key").map(str::trim).unwrap_or("");
    if key.len() != 24 {
        return Err(Error::new(400, "Bad Sec-WebSocket-Key"));
    }
    crate::rt::check_origin(cx)?;
    Ok(accept(key))
}

/// `sec-websocket-accept` for `key` (RFC 6455 §4.2.2).
fn accept(key: &str) -> String {
    let mut h = Sha1::new();
    h.update(key.as_bytes());
    h.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    base64(&h.finish())
}

/// Standard base64 with padding.
fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = c
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= c.len() {
                ABC[(n >> (18 - 6 * i) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}

/// SHA-1, FIPS 180-4. Only for the handshake, which the RFC fixes; it
/// protects nothing.
struct Sha1 {
    state: [u32; 5],
    block: [u8; 64],
    filled: usize,
    total: u64,
}

impl Sha1 {
    fn new() -> Sha1 {
        Sha1 {
            state: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476, 0xc3d2e1f0],
            block: [0; 64],
            filled: 0,
            total: 0,
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.total += data.len() as u64;
        while !data.is_empty() {
            let n = data.len().min(64 - self.filled);
            self.block[self.filled..self.filled + n].copy_from_slice(&data[..n]);
            self.filled += n;
            data = &data[n..];
            if self.filled == 64 {
                self.compress();
                self.filled = 0;
            }
        }
    }

    fn finish(mut self) -> [u8; 20] {
        let bits = self.total * 8;
        self.update(&[0x80]);
        while self.filled != 56 {
            self.update(&[0]);
        }
        self.update(&bits.to_be_bytes());
        let mut out = [0u8; 20];
        for (o, s) in out.chunks_mut(4).zip(self.state) {
            o.copy_from_slice(&s.to_be_bytes());
        }
        out
    }

    fn compress(&mut self) {
        let mut w = [0u32; 80];
        for (i, word) in self.block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = self.state;
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..20 => ((b & c) | (!b & d), 0x5a827999),
                20..40 => (b ^ c ^ d, 0x6ed9eba1),
                40..60 => ((b & c) | (b & d) | (c & d), 0x8f1bbcdc),
                _ => (b ^ c ^ d, 0xca62c1d6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            (e, d, c, b, a) = (d, c, b.rotate_left(30), a, t);
        }
        for (s, v) in self.state.iter_mut().zip([a, b, c, d, e]) {
            *s = s.wrapping_add(v);
        }
    }
}

const CONTINUATION: u8 = 0;
const TEXT: u8 = 1;
const BINARY: u8 = 2;
const CLOSE: u8 = 8;
const PING: u8 = 9;
const PONG: u8 = 10;

/// Close codes (RFC 6455 §7.4.1).
const NORMAL: u16 = 1000;
const GOING_AWAY: u16 = 1001;
const PROTOCOL_ERROR: u16 = 1002;
const NOT_UTF8: u16 = 1007;
const TOO_BIG: u16 = 1009;
const SERVER_ERROR: u16 = 1011;

/// A frame from the server: final, unmasked.
fn frame(w: &mut Vec<u8>, op: u8, payload: &[u8]) {
    w.push(0x80 | op);
    match payload.len() {
        n @ 0..126 => w.push(n as u8),
        n @ 126..=0xffff => {
            w.push(126);
            w.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            w.push(127);
            w.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    w.extend_from_slice(payload);
}

/// A frame parsed in place; it ends where its payload does.
struct Frame {
    fin: bool,
    op: u8,
    payload: std::ops::Range<usize>,
}

/// One frame from the client at the start of `b`, unmasked in place.
/// `Ok(None)` wants more bytes; `Err` is the close code to fail the
/// connection with.
fn parse(b: &mut [u8], limit: usize) -> std::result::Result<Option<Frame>, u16> {
    if b.len() < 2 {
        return Ok(None);
    }
    let (fin, op) = (b[0] & 0x80 != 0, b[0] & 0x0f);
    if b[0] & 0x70 != 0
        || !matches!(op, CONTINUATION | TEXT | BINARY | CLOSE | PING | PONG)
        || b[1] & 0x80 == 0
    {
        return Err(PROTOCOL_ERROR); // extension bits, unknown opcode, or unmasked
    }
    let (len, at) = match b[1] & 0x7f {
        n @ 0..126 => (n as u64, 2),
        126 if b.len() >= 4 => (u16::from_be_bytes([b[2], b[3]]) as u64, 4),
        127 if b.len() >= 10 => (u64::from_be_bytes(b[2..10].try_into().unwrap()), 10),
        _ => return Ok(None),
    };
    if len >> 63 != 0 || (op >= CLOSE && (!fin || len > 125)) {
        return Err(PROTOCOL_ERROR);
    }
    if len > limit as u64 {
        return Err(TOO_BIG);
    }
    let start: usize = at + 4;
    let end = start.saturating_add(len as usize);
    if b.len() < end {
        return Ok(None);
    }
    let mask = [b[at], b[at + 1], b[at + 2], b[at + 3]];
    for (i, byte) in b[start..end].iter_mut().enumerate() {
        *byte ^= mask[i & 3];
    }
    Ok(Some(Frame {
        fin,
        op,
        payload: start..end,
    }))
}

/// What the client sent, a frame at a time.
enum Event {
    Message(Message),
    Ping(Vec<u8>),
    /// The client closed; the payload to echo.
    Close(Vec<u8>),
    /// The client broke the protocol; the code to close with.
    Fail(u16),
}

/// Bytes from the client, turned into [`Event`]s.
struct Inbox {
    buf: Vec<u8>,
    /// Where the unparsed bytes in `buf` start; the rest is dropped only
    /// when more must be read, so many small frames cost no copying.
    at: usize,
    /// A fragmented message so far: its opcode and data.
    partial: Option<(u8, Vec<u8>)>,
    limit: usize,
}

impl Inbox {
    /// The next event in `buf`, or `None` until more bytes come.
    fn next(&mut self) -> Option<Event> {
        loop {
            let Frame { fin, op, payload } = match parse(&mut self.buf[self.at..], self.limit) {
                Ok(Some(f)) => f,
                Ok(None) => {
                    self.buf.drain(..self.at);
                    self.at = 0;
                    return None;
                }
                Err(code) => return Some(Event::Fail(code)),
            };
            let data = self.buf[self.at + payload.start..self.at + payload.end].to_vec();
            self.at += payload.end;
            let (op, data) = match (op, self.partial.take()) {
                (PING, partial) => {
                    self.partial = partial;
                    return Some(Event::Ping(data));
                }
                (PONG, partial) => {
                    self.partial = partial;
                    continue;
                }
                (CLOSE, _) => {
                    // No payload, or a code a peer may send and UTF-8 text.
                    let ok = match *data {
                        [] => true,
                        [a, b, ref reason @ ..] => {
                            matches!(u16::from_be_bytes([a, b]), 1000..=1003 | 1007..=1014 | 3000..=4999)
                                && std::str::from_utf8(reason).is_ok()
                        }
                        _ => false,
                    };
                    return Some(if ok {
                        Event::Close(data)
                    } else {
                        Event::Fail(PROTOCOL_ERROR)
                    });
                }
                (CONTINUATION, Some((first, mut so_far))) => {
                    if so_far.len() + data.len() > self.limit {
                        return Some(Event::Fail(TOO_BIG));
                    }
                    so_far.extend_from_slice(&data);
                    (first, so_far)
                }
                (TEXT | BINARY, None) => (op, data),
                _ => return Some(Event::Fail(PROTOCOL_ERROR)), // out of order
            };
            if !fin {
                self.partial = Some((op, data));
                continue;
            }
            return Some(match op {
                TEXT => match String::from_utf8(data) {
                    Ok(t) => Event::Message(Message::Text(t)),
                    Err(_) => Event::Fail(NOT_UTF8),
                },
                _ => Event::Message(Message::Binary(data)),
            });
        }
    }
}

/// An open WebSocket, for the handler of a [`Response::websocket`]. `recv`
/// and `send` may run at once, from `tokio::select!` or two tasks (a
/// clone is the same socket).
#[derive(Clone)]
pub struct WebSocket(Conn);

#[cfg(not(target_arch = "wasm32"))]
type Conn = std::sync::Arc<native::Conn>;
/// No host upgrades in the edge build, so there is never one.
#[cfg(target_arch = "wasm32")]
type Conn = std::convert::Infallible;

#[cfg(target_arch = "wasm32")]
impl WebSocket {
    pub async fn recv(&self) -> Option<Message> {
        match self.0 {}
    }

    pub async fn send(&self, msg: impl Into<Message>) -> std::result::Result<(), Gone> {
        let _ = msg;
        match self.0 {}
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use native::serve;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use crate::http;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpStream;
    use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
    use tokio::sync::Mutex;
    use tokio::time::Instant;

    pub struct Conn {
        read: Mutex<Reader>,
        write: Mutex<(OwnedWriteHalf, Vec<u8>)>,
        closed: AtomicBool,
    }

    /// The reading side, with when the client was last heard from and
    /// whether it has been pinged since.
    struct Reader {
        half: OwnedReadHalf,
        inbox: Inbox,
        heard: Instant,
        pinged: bool,
    }

    impl WebSocket {
        /// The next message, or `None` once the client has closed or gone, or
        /// the server is stopping. Pings are answered meanwhile. A client
        /// quiet for half of `WISP_WS_IDLE` is pinged, and one quiet for all
        /// of it is closed with 1001.
        pub async fn recv(&self) -> Option<Message> {
            let idle = crate::settings().ws_idle;
            let mut guard = self.0.read.lock().await;
            let Reader {
                half,
                inbox,
                heard,
                pinged,
            } = &mut *guard;
            loop {
                if self.0.closed.load(Ordering::Relaxed) {
                    return None;
                }
                match inbox.next() {
                    Some(Event::Message(m)) => return Some(m),
                    Some(Event::Ping(p)) => {
                        let _ = self.0.frame(PONG, &p).await;
                        continue;
                    }
                    Some(Event::Close(p)) => {
                        let _ = self.0.frame(CLOSE, &p).await;
                        return None;
                    }
                    Some(Event::Fail(code)) => {
                        let _ = self.0.frame(CLOSE, &code.to_be_bytes()).await;
                        return None;
                    }
                    None => {}
                }
                // Deadlines from `heard`, so a `recv` dropped by a
                // `select!` does not restart the clock.
                if !idle.is_zero() && heard.elapsed() >= idle {
                    let _ = self.0.frame(CLOSE, &GOING_AWAY.to_be_bytes()).await;
                    return None;
                }
                if !idle.is_zero() && !*pinged && heard.elapsed() >= idle / 2 {
                    *pinged = true;
                    let _ = self.0.frame(PING, b"").await;
                    continue;
                }
                let wait =
                    http::first(async { Some(half.read_buf(&mut inbox.buf).await) }, async {
                        http::stopped().await;
                        None
                    });
                let read = if idle.is_zero() {
                    wait.await
                } else {
                    let at = *heard + if *pinged { idle } else { idle / 2 };
                    match tokio::time::timeout_at(at, wait).await {
                        Ok(read) => read,
                        Err(_) => continue, // time to ping or close
                    }
                };
                match read {
                    Some(Ok(n)) if n > 0 => {
                        *heard = Instant::now();
                        *pinged = false;
                    }
                    Some(_) => {
                        self.0.closed.store(true, Ordering::Relaxed);
                        return None;
                    }
                    None => {
                        let _ = self.0.frame(CLOSE, &GOING_AWAY.to_be_bytes()).await;
                        return None;
                    }
                }
            }
        }

        /// Sends `msg`: a `String` or `&str` as text, a `Vec<u8>` or
        /// `&[u8]` as binary. Fails once the connection is closed or the
        /// server is stopping: stop sending then.
        pub async fn send(&self, msg: impl Into<Message>) -> std::result::Result<(), Gone> {
            match msg.into() {
                Message::Text(t) => self.0.frame(TEXT, t.as_bytes()).await,
                Message::Binary(b) => self.0.frame(BINARY, &b).await,
            }
        }
    }

    impl Conn {
        /// Writes one frame. A close is sent once; after it nothing is. A
        /// stopping server sends its close instead of anything else.
        async fn frame(&self, op: u8, payload: &[u8]) -> std::result::Result<(), Gone> {
            let mut guard = self.write.lock().await;
            if self.closed.load(Ordering::Relaxed) {
                return Err(Gone);
            }
            let stop = op != CLOSE && http::stopping();
            let going_away = GOING_AWAY.to_be_bytes();
            let (op, payload) = if stop {
                (CLOSE, &going_away[..])
            } else {
                (op, payload)
            };
            let (half, buf) = &mut *guard;
            buf.clear();
            frame(buf, op, payload);
            let sent = http::write(half, buf).await;
            if op == CLOSE || sent.is_err() {
                self.closed.store(true, Ordering::Relaxed);
            }
            if stop || sent.is_err() {
                Err(Gone)
            } else {
                Ok(())
            }
        }
    }

    /// Runs the handler on an upgraded connection. `early` is what the
    /// client sent after its handshake; `limit` the largest message.
    pub(crate) async fn serve(
        stream: TcpStream,
        early: Vec<u8>,
        limit: usize,
        upgrade: Upgrade,
        path: &str,
    ) {
        let (read, write) = stream.into_split();
        let conn = Arc::new(Conn {
            read: Mutex::new(Reader {
                half: read,
                inbox: Inbox {
                    buf: early,
                    at: 0,
                    partial: None,
                    limit,
                },
                heard: Instant::now(),
                pinged: false,
            }),
            write: Mutex::new((write, Vec::new())),
            closed: AtomicBool::new(false),
        });
        let result = http::catch((upgrade.0)(WebSocket(conn.clone()))).await;
        let code = match result {
            Ok(()) => NORMAL,
            Err(e) => {
                http::log(format_args!("wisp: WebSocket {path}: {}", e.detail()));
                SERVER_ERROR
            }
        };
        let _ = conn.frame(CLOSE, &code.to_be_bytes()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn sha1(data: &[u8]) -> String {
        let mut h = Sha1::new();
        h.update(data);
        hex(&h.finish())
    }

    #[test]
    fn sha1_vectors() {
        assert_eq!(sha1(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(sha1(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            sha1(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
        assert_eq!(
            sha1(&[b'a'; 1_000_000]),
            "34aa973cd4c4daa4f61eeb2bdbad27316534016f"
        );
    }

    #[test]
    fn handshake_matches_rfc_6455() {
        assert_eq!(
            accept("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"f"), "Zg==");
    }

    /// A frame as a client sends it: masked.
    fn client(fin: bool, op: u8, payload: &[u8]) -> Vec<u8> {
        let mask = [0x37, 0xfa, 0x21, 0x3d];
        let mut w = vec![if fin { 0x80 | op } else { op }];
        match payload.len() {
            n @ 0..126 => w.push(0x80 | n as u8),
            n @ 126..=0xffff => {
                w.push(0x80 | 126);
                w.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                w.push(0x80 | 127);
                w.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        w.extend_from_slice(&mask);
        w.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i & 3]));
        w
    }

    fn inbox(bytes: Vec<u8>, limit: usize) -> Inbox {
        Inbox {
            buf: bytes,
            at: 0,
            partial: None,
            limit,
        }
    }

    #[test]
    fn server_frames() {
        let mut w = Vec::new();
        frame(&mut w, TEXT, b"Hello");
        assert_eq!(w, [0x81, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f]); // RFC 6455 §5.7
        w.clear();
        frame(&mut w, BINARY, &[0; 256]);
        assert_eq!(&w[..4], [0x82, 0x7e, 0x01, 0x00]);
        w.clear();
        frame(&mut w, BINARY, &[0; 65536]);
        assert_eq!(&w[..10], [0x82, 0x7f, 0, 0, 0, 0, 0, 1, 0, 0]);
    }

    #[test]
    fn masked_message_from_the_rfc() {
        // RFC 6455 §5.7: a masked "Hello".
        let mut i = inbox(
            vec![
                0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58,
            ],
            100,
        );
        assert!(matches!(i.next(), Some(Event::Message(Message::Text(t))) if t == "Hello"));
        assert!(i.next().is_none() && i.buf.is_empty());
    }

    #[test]
    fn fragments_pings_and_partial_reads() {
        let mut bytes = client(false, TEXT, b"Hel");
        bytes.extend(client(true, PING, b"hi")); // a control frame between fragments
        bytes.extend(client(true, CONTINUATION, b"lo"));
        bytes.extend(client(true, BINARY, &[7; 300]));
        let mut i = inbox(Vec::new(), 1000);
        let mut got = Vec::new();
        for b in bytes {
            i.buf.push(b); // a byte at a time
            while let Some(e) = i.next() {
                got.push(e);
            }
        }
        assert!(matches!(&got[0], Event::Ping(p) if p == b"hi"));
        assert!(matches!(&got[1], Event::Message(Message::Text(t)) if t == "Hello"));
        assert!(matches!(&got[2], Event::Message(Message::Binary(b)) if b.len() == 300));
        assert_eq!(got.len(), 3);
    }

    #[test]
    fn close_is_echoed() {
        let mut i = inbox(client(true, CLOSE, &[0x03, 0xe8, b'b', b'y', b'e']), 100);
        assert!(matches!(i.next(), Some(Event::Close(p)) if p == [0x03, 0xe8, b'b', b'y', b'e']));
        let mut i = inbox(client(true, CLOSE, &[]), 100);
        assert!(matches!(i.next(), Some(Event::Close(p)) if p.is_empty()));
        let mut i = inbox(client(true, CLOSE, &[3]), 100);
        assert!(matches!(i.next(), Some(Event::Fail(PROTOCOL_ERROR))));
        // 1005 and 1006 are never sent; 999 is not a code.
        for code in [999u16, 1005, 1006, 2000] {
            let mut i = inbox(client(true, CLOSE, &code.to_be_bytes()), 100);
            assert!(
                matches!(i.next(), Some(Event::Fail(PROTOCOL_ERROR))),
                "{code}"
            );
        }
        let mut i = inbox(client(true, CLOSE, &[0x03, 0xe8, 0xff]), 100);
        assert!(matches!(i.next(), Some(Event::Fail(PROTOCOL_ERROR))));
    }

    #[test]
    fn protocol_errors() {
        let fail = |bytes: Vec<u8>, limit| match inbox(bytes, limit).next() {
            Some(Event::Fail(code)) => code,
            _ => 0,
        };
        let mut unmasked = Vec::new();
        frame(&mut unmasked, TEXT, b"hi");
        assert_eq!(fail(unmasked, 100), PROTOCOL_ERROR);
        let mut rsv = client(true, TEXT, b"hi");
        rsv[0] |= 0x40;
        assert_eq!(fail(rsv, 100), PROTOCOL_ERROR);
        assert_eq!(fail(client(true, 3, b""), 100), PROTOCOL_ERROR); // reserved opcode
        assert_eq!(fail(client(false, PING, b""), 100), PROTOCOL_ERROR); // fragmented control
        assert_eq!(fail(client(true, PING, &[0; 126]), 1000), PROTOCOL_ERROR);
        assert_eq!(fail(client(true, CONTINUATION, b"x"), 100), PROTOCOL_ERROR);
        let mut twice = client(false, TEXT, b"a");
        twice.extend(client(true, TEXT, b"b"));
        assert_eq!(fail(twice, 100), PROTOCOL_ERROR);
        assert_eq!(fail(client(true, TEXT, &[0xff]), 100), NOT_UTF8);
        // Too big: refused from the header, before the payload arrives.
        assert_eq!(
            fail(client(true, BINARY, &[0; 200])[..4].to_vec(), 100),
            TOO_BIG
        );
        let mut pieces = client(false, BINARY, &[0; 60]);
        pieces.extend(client(true, CONTINUATION, &[0; 60]));
        assert_eq!(fail(pieces, 100), TOO_BIG);
        // A 64-bit length with the top bit set.
        assert_eq!(
            fail(vec![0x82, 0xff, 0x80, 0, 0, 0, 0, 0, 0, 0], usize::MAX),
            PROTOCOL_ERROR
        );
    }

    /// What an event says, to compare.
    fn seen(e: &Event) -> (u8, Vec<u8>) {
        match e {
            Event::Message(Message::Text(t)) => (TEXT, t.as_bytes().to_vec()),
            Event::Message(Message::Binary(b)) => (BINARY, b.clone()),
            Event::Ping(p) => (PING, p.clone()),
            Event::Close(p) => (CLOSE, p.clone()),
            Event::Fail(code) => (0, code.to_be_bytes().to_vec()),
        }
    }

    #[test]
    fn frames_in_any_pieces_read_the_same() {
        use crate::fuzz::{Rng, mutate};
        let limit = 300;
        let mut rng = Rng::new(40);
        for _ in 0..2000 {
            // Messages, some in fragments with pings between, then a close.
            let mut wire = Vec::new();
            let mut want = Vec::new();
            for _ in 0..rng.below(5) {
                let text = rng.one_in(2);
                let data = if text {
                    rng.text(100).into_bytes()
                } else {
                    let n = rng.pick(&[0, 1, 125, 126, 200, limit]);
                    rng.bytes(n, b"")
                };
                let pieces = 1 + rng.below(3);
                let mut at = 0;
                for k in 0..pieces {
                    let end = if k + 1 == pieces {
                        data.len()
                    } else {
                        at + rng.below(data.len() - at + 1)
                    };
                    let op = if k == 0 {
                        if text { TEXT } else { BINARY }
                    } else {
                        CONTINUATION
                    };
                    wire.extend(client(k + 1 == pieces, op, &data[at..end]));
                    at = end;
                    if k + 1 == pieces {
                        want.push((if text { TEXT } else { BINARY }, data.clone()));
                    }
                    if rng.one_in(3) {
                        wire.extend(client(true, PING, b"p"));
                        want.push((PING, b"p".to_vec()));
                    }
                }
            }
            wire.extend(client(true, CLOSE, &NORMAL.to_be_bytes()));
            want.push((CLOSE, NORMAL.to_be_bytes().to_vec()));

            let mut i = inbox(Vec::new(), limit);
            let mut got = Vec::new();
            let mut left = &wire[..];
            while !left.is_empty() {
                let n = (1 + rng.below(40)).min(left.len());
                i.buf.extend_from_slice(&left[..n]);
                left = &left[n..];
                while let Some(e) = i.next() {
                    got.push(seen(&e));
                }
            }
            assert_eq!(got, want);

            // Broken: never a panic, never more held than a message allows.
            let mut broken = wire.clone();
            mutate(&mut rng, &mut broken);
            let mut i = inbox(Vec::new(), limit);
            'feed: for piece in broken.chunks(1 + rng.below(64)) {
                i.buf.extend_from_slice(piece);
                while let Some(e) = i.next() {
                    if let Event::Fail(_) | Event::Close(_) = e {
                        break 'feed; // the connection ends here
                    }
                }
                let partial = i.partial.as_ref().map_or(0, |p| p.1.len());
                assert!(partial <= limit && i.buf.len() - i.at <= limit + 14 + 64);
            }
        }
    }
}
