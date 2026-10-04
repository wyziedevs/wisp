//! The wasm build's connection driver (see [`Raw`]): the built-in server's
//! parser, deciding and serializing on bytes an edge host hands over. A child
//! of `http`, whose private parts it uses; `edge.rs` has the host's side.

use super::*;

/// The host's clock, in whole seconds.
fn now() -> u64 {
    crate::edge::clock().as_secs()
}

/// Buffers for a request from `peer`, the warmest set a thread has.
fn take(peer: SocketAddr) -> Box<Buffers> {
    let mut b = POOL.with_borrow_mut(Vec::pop).unwrap_or_else(|| {
        Box::new(Buffers {
            cx: Cx::new(peer),
            wbuf: Vec::with_capacity(16 * 1024),
            out: Out::default(),
            reply: Reply::default(),
        })
    });
    b.cx.wire.peer = peer;
    b
}

/// `b` emptied, for the next request on this thread.
fn give(mut b: Box<Buffers>) {
    b.cx.wire.buf.clear();
    b.wbuf.clear();
    trim(&mut b);
    POOL.with_borrow_mut(|p| {
        if p.len() < POOLED {
            p.push(b);
        }
    });
}

/// What one large request or response grew `b` to goes.
fn trim(b: &mut Buffers) {
    if b.cx.wire.buf.is_empty() && !policy::kept(b.cx.wire.buf.capacity()) {
        b.cx.wire.buf.shrink_to(8 * 1024);
    }
    if !policy::kept(b.wbuf.capacity()) {
        b.wbuf.shrink_to(KEEP_CAPACITY);
    }
    if !policy::kept(b.out.body.capacity()) {
        b.out = Out::default();
    }
}

/// What a [`Raw`] connection wants done next, once its buffered requests are
/// answered (what they wrote is in [`Raw::out`]: send it first).
pub(crate) enum Step {
    /// The write buffer is full: send it, then step again.
    Flush,
    /// Nothing whole is left to answer: wait for bytes.
    Idle,
    /// Send what is written, then close.
    Close,
    /// A streamed body follows what is written, as the client takes it.
    Stream(Stream),
    /// The 101 is written: the rest of the connection is a WebSocket's.
    Upgrade(Box<Upgraded>),
}

/// A connection upgraded to a WebSocket: what runs on it, and what the
/// client sent after its handshake.
pub(crate) struct Upgraded {
    pub(crate) upgrade: crate::ws::Upgrade,
    pub(crate) early: Vec<u8>,
    pub(crate) limit: usize,
    pub(crate) path: String,
}

/// A streamed body, to send as it comes: framed in chunks (HTTP/1.1) or
/// ending with the connection, which then closes.
pub(crate) struct Stream {
    pub(crate) body: tokio::sync::mpsc::Receiver<Vec<u8>>,
    pub(crate) chunked: bool,
    pub(crate) close: bool,
}

/// A connection an edge host reads and writes (Node's sockets, Bun's,
/// Deno's) and Wisp drives: the built-in server's parser, deciding and
/// serializing, on bytes the host hands over. Pipelining, keep-alive,
/// chunked bodies, limits and refusals are the parser's, as on the wire.
/// It holds buffers only while bytes are in them.
pub(crate) struct Raw {
    peer: SocketAddr,
    b: Option<Box<Buffers>>,
    continued: bool,
    /// When the unfinished request's head, and its body, started coming.
    head_since: Option<u64>,
    body_since: Option<u64>,
    /// When the rest of it must have come by, as on the wire: a client that
    /// sends a request a byte a minute is refused when the next comes late.
    /// The host closes a connection quiet for a minute.
    deadline: Option<u64>,
}

impl Raw {
    pub(crate) fn new(peer: SocketAddr) -> Raw {
        Raw {
            peer,
            b: None,
            continued: false,
            head_since: None,
            body_since: None,
            deadline: None,
        }
    }

    /// Bytes the client sent.
    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        let peer = self.peer;
        let b = self.b.get_or_insert_with(|| take(peer));
        b.cx.wire.buf.extend_from_slice(bytes);
    }

    /// What is to be sent to the client.
    pub(crate) fn out(&self) -> &[u8] {
        self.b.as_ref().map_or(&[], |b| &b.wbuf)
    }

    /// [`Raw::out`] is sent.
    pub(crate) fn sent(&mut self) {
        if let Some(b) = &mut self.b {
            b.wbuf.clear();
        }
    }

    /// A chunk of a streamed body, framed as `chunked` says, written out.
    pub(crate) fn frame(&mut self, chunked: bool, chunk: &[u8]) {
        let Some(b) = &mut self.b else { return };
        if chunked {
            push_hex(&mut b.wbuf, chunk.len() as u64);
            b.wbuf.extend_from_slice(b"\r\n");
            b.wbuf.extend_from_slice(chunk);
            b.wbuf.extend_from_slice(b"\r\n");
        } else {
            b.wbuf.extend_from_slice(chunk);
        }
    }

    pub(crate) fn end_stream(&mut self, chunked: bool) {
        if let (Some(b), true) = (&mut self.b, chunked) {
            b.wbuf.extend_from_slice(b"0\r\n\r\n");
        }
    }

    /// The buffers go back to the pool while nothing is in them.
    pub(crate) fn rest(&mut self) {
        if self
            .b
            .as_ref()
            .is_some_and(|b| b.cx.wire.buf.is_empty() && b.wbuf.is_empty())
            && let Some(b) = self.b.take()
        {
            give(b);
        }
    }

    /// An answer of `status` alone, and the connection closing.
    pub(crate) fn refuse<A: App>(&mut self, status: u16) -> Step {
        let peer = self.peer;
        let b = self.b.get_or_insert_with(|| take(peer));
        let Buffers {
            wbuf, out, reply, ..
        } = &mut **b;
        reply.set_plain(status, reason(status));
        serialize::<A, true>(wbuf, reply, out, true, false, false);
        Step::Close
    }

    /// Answers the whole requests in what the client sent, into [`Raw::out`].
    pub(crate) async fn step<A: App>(&mut self) -> Step {
        if self.deadline.is_some_and(|d| now() > d) {
            self.deadline = None;
            return self.refuse::<A>(408);
        }
        let Some(b) = self.b.as_mut() else {
            return Step::Idle;
        };
        let mut used = 0;
        let step = loop {
            if used >= b.cx.wire.buf.len() {
                break Step::Idle;
            }
            if b.wbuf.len() >= KEEP_CAPACITY {
                break Step::Flush;
            }
            let parsed = parse::<A>(&mut b.cx, used, true);
            let Buffers {
                cx,
                wbuf,
                out,
                reply,
            } = &mut **b;
            match parsed {
                Parsed::Request(req) => {
                    decide::<A>(cx, out, reply, req.routed).await;
                    if !crate::edge_store::Saved.await {
                        *reply = Reply::plain(500);
                    }
                    // Only as a 101: a hook may have answered otherwise.
                    let upgrade = reply.take_websocket().filter(|_| reply.status == 101);
                    let streamed = serialize::<A, true>(
                        wbuf,
                        reply,
                        out,
                        cx.wire.http11,
                        req.keep_alive || upgrade.is_some(),
                        cx.method == Method::Head,
                    );
                    cx.reset();
                    used += req.len;
                    self.continued = false;
                    (self.head_since, self.body_since, self.deadline) = (None, None, None);
                    if let Some(upgrade) = upgrade {
                        // The rest of the connection is the WebSocket's.
                        let early = cx.wire.buf[used..].to_vec();
                        let (limit, path) = (body_limit::<A>(cx.path()), cx.path().to_owned());
                        used = cx.wire.buf.len();
                        break Step::Upgrade(Box::new(Upgraded {
                            upgrade,
                            early,
                            limit,
                            path,
                        }));
                    }
                    if let Some(s) = streamed {
                        break Step::Stream(Stream {
                            body: s.body,
                            chunked: s.chunked,
                            close: s.close,
                        });
                    }
                    if !req.keep_alive {
                        break Step::Close;
                    }
                }
                Parsed::Partial {
                    expect_continue,
                    body,
                    ..
                } => {
                    let (now, have) = (now(), cx.wire.buf.len() - used);
                    self.deadline = Some(match body {
                        true => {
                            policy::body_deadline(now, *self.body_since.get_or_insert(now), have)
                        }
                        false => policy::head_deadline(*self.head_since.get_or_insert(now)),
                    });
                    if expect_continue && !self.continued {
                        wbuf.extend_from_slice(b"HTTP/1.1 100 Continue\r\n\r\n");
                        self.continued = true;
                    }
                    break Step::Idle;
                }
                Parsed::Invalid(status) => {
                    reply.set_plain(status, reason(status));
                    serialize::<A, true>(wbuf, reply, out, true, false, false);
                    break Step::Close;
                }
            }
        };
        b.cx.wire.buf.drain(..used);
        trim(b);
        step
    }
}
