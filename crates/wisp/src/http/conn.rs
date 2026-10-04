//! A connection: its socket, its buffers and the loop that answers its requests.

use super::*;

/// A connection's socket: tokio's, or on Linux one on its worker's io_uring
/// or epoll.
#[cfg(not(target_arch = "wasm32"))]
pub(super) enum Conn {
    Tcp(TcpStream),
    #[cfg(target_os = "linux")]
    Ring(crate::uring::Sock),
    #[cfg(target_os = "linux")]
    Poll(crate::epoll::Sock),
}

#[cfg(not(target_arch = "wasm32"))]
impl Conn {
    /// Reads what came onto the end of `buf`: `Ok(0)` when the peer closed.
    /// Safe to drop unfinished.
    pub(super) async fn read(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        match self {
            Conn::Tcp(s) => s.read_buf(buf).await,
            #[cfg(target_os = "linux")]
            Conn::Ring(s) => s.read(buf).await,
            #[cfg(target_os = "linux")]
            Conn::Poll(s) => s.read(buf).await,
        }
    }

    /// Writes all of `buf` (see [`write`]) and leaves it empty. On the ring
    /// it is only queued, to go with the other connections' sends; on the
    /// epoll what the socket has no room for goes on without the caller. A
    /// failure then shows in the next call.
    pub(super) async fn write(&mut self, buf: &mut Vec<u8>) -> io::Result<()> {
        match self {
            Conn::Tcp(s) => {
                write(s, buf).await?;
                buf.clear();
                Ok(())
            }
            #[cfg(target_os = "linux")]
            Conn::Ring(s) => s.write(buf).await,
            #[cfg(target_os = "linux")]
            Conn::Poll(s) => s.write(buf).await,
        }
    }

    /// What came onto the end of `buf`, without waiting: `None` when nothing
    /// has (a wakeup with nothing behind it), else as [`Conn::read`]. Only
    /// where `waits_bare`.
    pub(super) fn try_read(&mut self, buf: &mut Vec<u8>) -> Option<io::Result<usize>> {
        let read = match self {
            Conn::Tcp(s) => s.try_read_buf(buf),
            #[cfg(target_os = "linux")]
            Conn::Ring(_) => return None,
            #[cfg(target_os = "linux")]
            Conn::Poll(s) => s.try_read(buf),
        };
        match read {
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => None,
            r => Some(r),
        }
    }

    /// It can wait for something to read without a buffer to read it into
    /// (`readable`). The ring cannot: it receives into the connection's
    /// buffer as it waits.
    pub(super) fn waits_bare(&self) -> bool {
        match self {
            Conn::Tcp(_) => true,
            #[cfg(target_os = "linux")]
            Conn::Ring(_) => false,
            #[cfg(target_os = "linux")]
            Conn::Poll(_) => true,
        }
    }

    /// What the epoll driver received for this connection and left to it
    /// (see [`on_driver`]), once `readable`.
    pub(super) fn handed(&mut self) -> Option<Handed> {
        match self {
            #[cfg(target_os = "linux")]
            Conn::Poll(s) => s.handed(),
            _ => None,
        }
    }

    /// Ends the sending side, once all of it is sent.
    pub(super) async fn shutdown(&mut self) {
        match self {
            Conn::Tcp(s) => {
                let _ = s.shutdown().await;
            }
            #[cfg(target_os = "linux")]
            Conn::Ring(s) => s.shutdown().await,
            #[cfg(target_os = "linux")]
            Conn::Poll(s) => s.shutdown().await,
        }
    }

    /// The socket as tokio's, for a WebSocket, and what came after the
    /// handshake: `early`, then what the ring received that was not read.
    pub(super) async fn into_tcp(self, early: Vec<u8>) -> io::Result<(TcpStream, Vec<u8>)> {
        match self {
            Conn::Tcp(s) => Ok((s, early)),
            #[cfg(target_os = "linux")]
            Conn::Ring(s) => s.into_tcp(early).await,
            #[cfg(target_os = "linux")]
            Conn::Poll(s) => s.into_tcp(early).await,
        }
    }
}

/// Reads more into `buf`: `Ok(0)` when the peer closed, `TimedOut` if
/// nothing came by `deadline` (in `seconds()`).
///
/// `timer` is the connection's one timer. Deadlines are whole seconds, so
/// it is moved at most once a second, and a later deadline only updates it
/// in place: a timer made and dropped per read cost two lock round trips on
/// the timer wheel and a clock read, every request. On the epoll the
/// driver keeps the deadline instead, and `timer` goes unused.
#[cfg(not(target_arch = "wasm32"))]
pub(super) async fn read(
    stream: &mut Conn,
    buf: &mut Vec<u8>,
    timer: std::pin::Pin<&mut tokio::time::Sleep>,
    deadline: u64,
) -> io::Result<usize> {
    #[cfg(target_os = "linux")]
    if let Conn::Poll(s) = stream {
        return s.read_by(buf, deadline).await;
    }
    by(timer, deadline, stream.read(buf)).await
}

/// Waits until the socket has something to read (or its end, or an error,
/// which the read after it reports), failing with `TimedOut` if nothing
/// came by `deadline`, as [`read`] does. Only where `waits_bare`.
#[cfg(not(target_arch = "wasm32"))]
pub(super) async fn readable(
    stream: &mut Conn,
    timer: std::pin::Pin<&mut tokio::time::Sleep>,
    deadline: u64,
) -> io::Result<()> {
    match stream {
        #[cfg(target_os = "linux")]
        Conn::Poll(s) => s.readable_by(deadline).await,
        #[cfg(target_os = "linux")]
        Conn::Ring(_) => Ok(()),
        Conn::Tcp(s) => by(timer, deadline, s.readable()).await,
    }
}

/// `f`, unless `deadline` (in `seconds()`) comes first: then `TimedOut`.
#[cfg(not(target_arch = "wasm32"))]
pub(super) async fn by<T>(
    mut timer: std::pin::Pin<&mut tokio::time::Sleep>,
    deadline: u64,
    f: impl Future<Output = io::Result<T>>,
) -> io::Result<T> {
    let when = instant(deadline);
    if timer.deadline() != when {
        timer.as_mut().reset(when);
    }
    first(f, async {
        timer.await;
        Err(io::ErrorKind::TimedOut.into())
    })
    .await
}

/// Runs `f`, giving up with `TimedOut` after `limit`. A timer is only set
/// up when `f` cannot finish at once, which most writes can.
#[cfg(not(target_arch = "wasm32"))]
pub(super) async fn within<T>(
    limit: Duration,
    f: impl Future<Output = io::Result<T>>,
) -> io::Result<T> {
    let mut f = std::pin::pin!(f);
    if let Poll::Ready(done) = std::future::poll_fn(|cx| Poll::Ready(f.as_mut().poll(cx))).await {
        return done;
    }
    tokio::time::timeout(limit, f)
        .await
        .unwrap_or_else(|_| Err(io::ErrorKind::TimedOut.into()))
}

/// Writes all of `buf`, giving up on a client that has taken none of it for
/// `WRITE_TIMEOUT`, so one that stops reading cannot hold a connection (and
/// its buffers) forever. A slow client that keeps reading is fine.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn write(stream: &mut (impl AsyncWriteExt + Unpin), buf: &[u8]) -> io::Result<()> {
    let mut at = 0;
    while at < buf.len() {
        match within(WRITE_TIMEOUT, stream.write(&buf[at..])).await? {
            0 => return Err(io::ErrorKind::WriteZero.into()),
            n => at += n,
        }
    }
    Ok(())
}

/// A complete request, described by `cx`: its bytes, whether its client
/// asked to keep the connection open, and its route when a body's limit
/// had `parse` find it (its params are in `cx` then).
#[derive(Clone, Copy)]
pub(super) struct Req {
    pub(super) len: usize,
    pub(super) keep_alive: bool,
    pub(super) routed: Option<Option<usize>>,
}

pub(super) enum Parsed {
    Request(Req),
    /// More bytes are needed. `need` is the full request size when known;
    /// `body` is set once the head is in.
    Partial {
        need: usize,
        expect_continue: bool,
        body: bool,
    },
    /// Malformed or refused: answer with this status and close.
    Invalid(u16),
}

/// What a connection works in: its `Cx` (and read buffer), write buffer,
/// page and reply, all reused across its requests. Boxed, so a connection
/// waiting holds a pointer at most, and handing them on moves only that.
pub(super) struct Buffers {
    pub(super) cx: Cx,
    pub(super) wbuf: Vec<u8>,
    pub(super) out: Out,
    pub(super) reply: Reply,
}

/// How many `Buffers` a thread keeps for the requests to come.
pub(super) const POOLED: usize = 64;

thread_local! {
    /// Buffers no request is using, last returned first: a connection waits
    /// for its next request without any, and takes the warmest set when it
    /// comes. So a thread whose requests are answered as they arrive works
    /// in one set, in cache, however many connections it serves, and a new
    /// connection allocates nothing. Boxed: a set moves as a pointer.
    #[allow(clippy::vec_box)]
    pub(super) static POOL: std::cell::RefCell<Vec<Box<Buffers>>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Buffers for a request from `peer`.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn take_buffers(peer: SocketAddr) -> Box<Buffers> {
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

/// `b` back, for the next request on this thread. Each request reset its
/// `Cx` as it was answered.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn give_buffers(mut b: Box<Buffers>) {
    reset_buffers(&mut b);
    POOL.with_borrow_mut(|p| {
        if p.len() < POOLED {
            p.push(b);
        }
    });
}

/// `b` emptied of what came and what went, for its next request.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn reset_buffers(b: &mut Buffers) {
    b.cx.wire.buf.clear();
    b.wbuf.clear();
    trim_buffers(b);
}

/// What one large request or response grew `b` to goes; the read buffer
/// only once it is empty.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn trim_buffers(b: &mut Buffers) {
    if b.cx.wire.buf.is_empty() {
        policy::trim(&mut b.cx.wire.buf, READ_CAPACITY);
    }
    policy::trim(&mut b.wbuf, KEEP_CAPACITY);
    if !policy::kept(b.out.body.capacity()) {
        b.out = Out::default();
    }
    if !policy::kept_headers(b.reply.headers.capacity()) {
        b.reply.headers = Vec::new();
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) async fn connection<A: App>(stream: Conn, peer: SocketAddr) {
    let mut held = Some(take_buffers(peer));
    requests::<A>(stream, peer, &mut held).await;
    if let Some(b) = held {
        give_buffers(b);
    }
}

/// Answers the requests of one connection, until it closes.
#[cfg(not(target_arch = "wasm32"))]
pub(super) async fn requests<A: App>(
    mut stream: Conn,
    peer: SocketAddr,
    held: &mut Option<Box<Buffers>>,
) {
    let mut busy = Busy(false);
    let mut head_since: Option<u64> = None;
    let mut body_since: Option<u64> = None;
    let mut sent_continue = false;
    let mut timer = std::pin::pin!(tokio::time::sleep_until(instant(0)));
    // Where the driver stopped in what it handed over, and the request
    // there when it got that far (see `Handed`).
    let mut start = 0;
    let mut ahead: Option<Ahead> = None;

    'conn: loop {
        let b = &mut **held.get_or_insert_with(|| take_buffers(peer));
        let mut used = std::mem::take(&mut start);
        let mut need = 0;
        let mut in_body = false;
        let mut close = false;
        let mut refused = false;
        while used < b.cx.wire.buf.len() {
            let (parsed, decided) = match ahead.take() {
                Some((req, decided)) => (Parsed::Request(req), decided),
                None => (parse::<A>(&mut b.cx, used, true), false),
            };
            let Buffers {
                cx,
                wbuf,
                out,
                reply,
            } = &mut *b;
            match parsed {
                Parsed::Request(req) => {
                    let keep_alive = policy::keeps_open(req.keep_alive, stopping());
                    if !decided {
                        decide::<A>(cx, out, reply, req.routed).await;
                    }
                    // Only as a 101: a hook may have answered otherwise.
                    let upgrade = reply.take_websocket().filter(|_| reply.status == 101);
                    let streamed = serialize::<A, true>(
                        wbuf,
                        reply,
                        out,
                        cx.wire.http11,
                        keep_alive || upgrade.is_some(),
                        cx.method == Method::Head,
                    );
                    cx.reset();
                    used += req.len;
                    // Answers to many small pipelined requests go out in
                    // pieces, so a buffer of them cannot make a huge one.
                    if streamed.is_none()
                        && upgrade.is_none()
                        && wbuf.len() >= KEEP_CAPACITY
                        && stream.write(wbuf).await.is_err()
                    {
                        return;
                    }
                    if let Some(upgrade) = upgrade {
                        // The rest of the connection is the WebSocket's,
                        // on tokio's socket, with what the client sent
                        // after its handshake. It holds none of the
                        // buffers meanwhile.
                        let early = cx.wire.buf[used..].to_vec();
                        let (limit, path) = (body_limit::<A>(cx.path()), cx.path().to_owned());
                        let sent = stream.write(wbuf).await.is_ok();
                        give_buffers(held.take().expect("taken at the top of the loop"));
                        if sent && let Ok((tcp, early)) = stream.into_tcp(early).await {
                            crate::ws::serve(tcp, early, limit, upgrade, &path).await;
                        }
                        return;
                    }
                    head_since = None;
                    body_since = None;
                    sent_continue = false;
                    if let Some(s) = streamed {
                        // What was answered so far goes first, then the body
                        // as it comes, which may take hours (server-sent
                        // events): meanwhile the connection holds none of
                        // the buffers, and what the client sends (a request
                        // after this one) waits in `early`.
                        if stream.write(wbuf).await.is_err() {
                            return;
                        }
                        let mut early = cx.wire.buf[used..].to_vec();
                        give_buffers(held.take().expect("taken at the top of the loop"));
                        let mut w = Vec::new();
                        if pump(&mut stream, s.body, s.chunked, &mut w, &mut early)
                            .await
                            .is_err()
                        {
                            return;
                        }
                        if s.close {
                            stream.shutdown().await;
                            return;
                        }
                        held.insert(take_buffers(peer))
                            .cx
                            .wire
                            .buf
                            .extend_from_slice(&early);
                        continue 'conn;
                    }
                    if !keep_alive {
                        close = true;
                        break;
                    }
                }
                Parsed::Partial {
                    need: n,
                    expect_continue,
                    body,
                } => {
                    need = n;
                    in_body = body;
                    if policy::continues(expect_continue, sent_continue) {
                        wbuf.extend_from_slice(b"HTTP/1.1 100 Continue\r\n\r\n");
                        sent_continue = true;
                    }
                    break;
                }
                Parsed::Invalid(status) => {
                    // No HTTP/1 request, but HTTP/2's preface: an HTTP/1
                    // request never comes this way, so it pays nothing.
                    #[cfg(feature = "h2")]
                    if used == 0 && cx.wire.buf.starts_with(&h2::PREFACE[..18]) {
                        h2::serve::<A>(&mut stream, b, timer.as_mut()).await;
                        return;
                    }
                    reply.set_plain(status, reason(status));
                    serialize::<A, true>(wbuf, reply, out, true, false, false);
                    (close, refused) = (true, true);
                    break;
                }
            }
        }

        if !b.wbuf.is_empty() && stream.write(&mut b.wbuf).await.is_err() {
            return;
        }
        if close {
            stream.shutdown().await;
            if refused {
                linger(&mut stream, &mut b.cx.wire.buf, timer.as_mut()).await;
            }
            return;
        }

        b.cx.wire.buf.drain(..used);
        trim_buffers(b);
        let have = b.cx.wire.buf.len();
        b.cx.wire.buf.reserve(policy::read_ahead(need, have));

        let deadline = if have == 0 {
            busy.set(false);
            if stopping() {
                return;
            }
            policy::idle_deadline(seconds())
        } else if in_body {
            let since = *body_since.get_or_insert_with(seconds);
            policy::body_deadline(seconds(), since, have)
        } else {
            policy::head_deadline(*head_since.get_or_insert_with(seconds))
        };
        if have == 0 && stream.waits_bare() {
            // Nothing of a next request yet: wait for it without buffers,
            // which go to whichever request on this thread comes first.
            give_buffers(held.take().expect("taken at the top of the loop"));
            loop {
                if readable(&mut stream, timer.as_mut(), deadline)
                    .await
                    .is_err()
                {
                    return; // closed, error or too slow
                }
                if let Some(h) = stream.handed() {
                    busy.set(true);
                    *held = Some(match h.held {
                        Holding::Ready(b) => b,
                        Holding::Deciding(d) => decided(d).await,
                    });
                    (start, ahead) = (h.at, h.ahead);
                    continue 'conn;
                }
                let b = held.insert(take_buffers(peer));
                match stream.try_read(&mut b.cx.wire.buf) {
                    Some(Ok(n)) if n > 0 => break,
                    Some(_) => return,
                    // Readable with nothing to read (a new socket, which
                    // the epoll tries at once; tokio's on Windows, which
                    // says so of every new one): it waits again holding
                    // nothing, rather than in a read holding the buffers.
                    None => give_buffers(held.take().expect("just taken")),
                }
            }
            busy.set(true);
            continue;
        }
        match read(&mut stream, &mut b.cx.wire.buf, timer.as_mut(), deadline).await {
            Ok(n) if n > 0 => busy.set(true),
            _ => return, // closed, error or too slow
        }
    }
}

/// After a refused request: what the client still sends (the rest of a body
/// too large, say) is read and dropped for a moment, up to a limit. A socket
/// closed with bytes unread resets the connection, and the client may lose
/// the answer before it has read it.
#[cfg(not(target_arch = "wasm32"))]
pub(super) async fn linger(
    stream: &mut Conn,
    buf: &mut Vec<u8>,
    mut timer: std::pin::Pin<&mut tokio::time::Sleep>,
) {
    let deadline = seconds() + policy::LINGER.as_secs();
    let mut left = policy::LINGER_BYTES;
    while left > 0 {
        buf.clear();
        match read(stream, buf, timer.as_mut(), deadline).await {
            Ok(n) if n > 0 => left = left.saturating_sub(n),
            _ => return,
        }
    }
}

/// Sends a streamed body as it is made, until its sender is dropped or the
/// server stops (both end it properly), or the client goes (an error).
/// Chunks made meanwhile go out in one write. Anything the client sends
/// meanwhile is kept in `early`, up to a limit.
#[cfg(not(target_arch = "wasm32"))]
pub(super) async fn pump(
    stream: &mut Conn,
    mut body: mpsc::Receiver<Vec<u8>>,
    chunked: bool,
    w: &mut Vec<u8>,
    early: &mut Vec<u8>,
) -> io::Result<()> {
    enum Next {
        Chunk(Option<Vec<u8>>),
        Stop,
        Gone,
    }
    // An empty chunk would read as the end: it is left out.
    let put = |w: &mut Vec<u8>, chunk: &[u8]| {
        if chunked && !chunk.is_empty() {
            push_hex(w, chunk.len() as u64);
            w.extend_from_slice(b"\r\n");
            w.extend_from_slice(chunk);
            w.extend_from_slice(b"\r\n");
        } else {
            w.extend_from_slice(chunk);
        }
    };
    let mut stop = std::pin::pin!(stopped());
    loop {
        let next = first(
            async { Next::Chunk(body.recv().await) },
            first(
                async {
                    stop.as_mut().await;
                    Next::Stop
                },
                async {
                    loop {
                        match stream.read(early).await {
                            Ok(0) | Err(_) => return Next::Gone,
                            // More than a connection keeps, sent behind an
                            // answer still streaming: the connection ends.
                            // Not reading on would miss the client leaving,
                            // and hold a quiet stream open for good.
                            Ok(_) if early.len() > KEEP_CAPACITY => return Next::Gone,
                            Ok(_) => {}
                        }
                    }
                },
            ),
        )
        .await;
        w.clear();
        let mut end = match next {
            Next::Chunk(Some(chunk)) => {
                put(w, &chunk);
                false
            }
            Next::Chunk(None) | Next::Stop => true,
            Next::Gone => return Err(io::ErrorKind::ConnectionAborted.into()),
        };
        while !end && w.len() < KEEP_CAPACITY {
            match body.try_recv() {
                Ok(chunk) => put(w, &chunk),
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => end = true,
            }
        }
        if end && chunked {
            w.extend_from_slice(b"0\r\n\r\n");
        }
        if !w.is_empty() {
            stream.write(w).await?;
        }
        if end {
            return Ok(());
        }
    }
}
