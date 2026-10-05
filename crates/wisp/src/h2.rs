//! HTTP/2 over cleartext with prior knowledge (h2c, RFC 9113), behind the
//! `h2` feature. A client that opens with the connection preface gets an
//! HTTP/2 connection; every other byte stream stays HTTP/1.1, which pays
//! nothing for it: the preface is noticed only where the HTTP/1 parser
//! already refused the request (`PRI * HTTP/2.0` is no HTTP/1 request).
//!
//! A [`Session`] is the protocol, with no I/O: bytes in, frames out, and the
//! requests that are whole. [`serve`] drives one over a connection, and
//! answers each request through the same `Cx`, router and serializer as
//! HTTP/1: a stream's HEADERS and DATA become an HTTP/1.1 request
//! (`Cx::from_request`), and the HTTP/1.1 answer becomes HEADERS and DATA.
//!
//! Streams are answered at once: each stream's handler runs as its own
//! future on the connection, and DATA from every answer in flight is
//! interleaved a frame at a time, within the stream and connection
//! windows. Frames are read all the while, so WINDOW_UPDATE, PING, new
//! HEADERS and RST_STREAM (which drops that stream's handler or body) are
//! taken during a long stream. HPACK is our
//! own (static and dynamic tables, Huffman), so the feature adds no
//! dependency. Limits: 100 concurrent streams, 16 KiB header lists, 16 KiB
//! frames, a 4 KiB dynamic table, 1 MiB of bodies still coming (past it,
//! only the oldest stream's window opens again); a reset flood (the
//! client's, or one it draws from us), a CONTINUATION flood, an empty-frame
//! flood and an HPACK bomb each end the connection with GOAWAY. Nothing here panics: every index is checked, every error is a
//! frame.

use std::collections::VecDeque;

/// What a client sends first.
pub(crate) const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// The most streams a client may have open at once.
const MAX_STREAMS: usize = 100;
/// The largest header list, as RFC 9113 counts it (name + value + 32 each).
const MAX_HEADER_LIST: usize = 16 * 1024;
/// The largest frame payload we take (the protocol's least maximum).
const MAX_FRAME: usize = 16 * 1024;
/// Our HPACK dynamic table, the default.
const TABLE_SIZE: usize = 4096;
/// Resets a client may make beyond the streams it saw answered.
const RESET_BUDGET: u32 = 2 * MAX_STREAMS as u32;
/// Empty DATA, CONTINUATION or unknown frames, and PINGs and SETTINGS, a
/// client may send between two answers: work for no request.
const IDLE_FRAME_BUDGET: u32 = 1000;
/// Body bytes the streams still coming may hold before only the oldest of
/// them is given more window.
const MAX_HELD: usize = 1 << 20;
/// The default window.
const WINDOW: i64 = 65_535;
const MAX_WINDOW: i64 = (1 << 31) - 1;

// Frame types.
const DATA: u8 = 0;
const HEADERS: u8 = 1;
const PRIORITY: u8 = 2;
const RST_STREAM: u8 = 3;
const SETTINGS: u8 = 4;
const PUSH_PROMISE: u8 = 5;
const PING: u8 = 6;
const GOAWAY: u8 = 7;
const WINDOW_UPDATE: u8 = 8;
const CONTINUATION: u8 = 9;

// Flags.
const END_STREAM: u8 = 0x1;
const ACK: u8 = 0x1;
const END_HEADERS: u8 = 0x4;
const PADDED: u8 = 0x8;
const PRIORITY_FLAG: u8 = 0x20;

/// Error codes (RFC 9113 §7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Code {
    Protocol = 1,
    Internal = 2,
    FlowControl = 3,
    StreamClosed = 5,
    FrameSize = 6,
    Refused = 7,
    Compression = 9,
    Calm = 11,
}

/// A request whose stream ended: what [`serve`] answers.
#[derive(Debug, Default)]
pub(crate) struct Request {
    pub method: String,
    pub path: String,
    pub authority: String,
    pub headers: Vec<(String, Vec<u8>)>,
    pub body: Vec<u8>,
    /// Over the body limit: answered 413, its data dropped.
    pub too_large: bool,
}

/// An open stream.
struct Stream {
    id: u32,
    /// What we may still send on it.
    send: i64,
    /// What the client may still send on it (our window).
    recv: i64,
    /// The client ended its side: a request in `ready`, or answered.
    ended: bool,
    req: Request,
    /// `content-length`, when the client gave one.
    length: Option<u64>,
}

/// A header block coming in pieces (HEADERS, then CONTINUATION).
struct Block {
    id: u32,
    end_stream: bool,
    bytes: Vec<u8>,
    frames: u32,
    /// The stream was reset as its HEADERS came (it depends on itself):
    /// the block is decoded, for the table, and opens nothing.
    reset: bool,
}

/// One HTTP/2 connection's protocol state.
pub(crate) struct Session {
    hpack: Decoder,
    streams: Vec<Stream>,
    /// The highest stream the client opened.
    last: u32,
    block: Option<Block>,
    /// The client's settings that bind us.
    peer_window: i64,
    peer_frame: usize,
    /// What we may still send on the connection.
    send: i64,
    /// What the client may still send on the connection (our window).
    recv: i64,
    /// Whether the preface came.
    greeted: bool,
    resets: u32,
    /// Streams we ended: a reset one never counts, so a client cannot
    /// start handlers and cancel them past `RESET_BUDGET`.
    answered: u32,
    idle: u32,
    /// The largest body a request may have (per path, see `limit`).
    limit: fn(&str) -> usize,
    /// Frames to send.
    pub out: Vec<u8>,
    /// Streams whose request is whole, to answer in order.
    pub ready: VecDeque<u32>,
    /// GOAWAY sent: nothing more is read.
    pub done: bool,
}

/// Why a frame could not be taken.
enum Fault {
    Conn(Code),
    Stream(u32, Code),
}

impl Session {
    pub(crate) fn new(limit: fn(&str) -> usize) -> Session {
        let mut s = Session {
            hpack: Decoder::new(),
            streams: Vec::new(),
            last: 0,
            block: None,
            peer_window: WINDOW,
            peer_frame: MAX_FRAME,
            send: WINDOW,
            recv: WINDOW,
            greeted: false,
            resets: 0,
            answered: 0,
            idle: 0,
            limit,
            out: Vec::with_capacity(1024),
            ready: VecDeque::new(),
            done: false,
        };
        // Our settings: no push, the stream and header limits.
        let mut p = Vec::with_capacity(18);
        for (k, v) in [
            (2u16, 0u32),
            (3, MAX_STREAMS as u32),
            (6, MAX_HEADER_LIST as u32),
        ] {
            p.extend_from_slice(&k.to_be_bytes());
            p.extend_from_slice(&v.to_be_bytes());
        }
        s.frame(SETTINGS, 0, 0, &p);
        s
    }

    /// Takes the whole frames at the start of `buf`: how many bytes it used.
    /// What a frame asks for (an answer, an ack, an error) goes into `out`.
    pub(crate) fn feed(&mut self, buf: &[u8]) -> usize {
        let mut at = 0;
        if !self.greeted {
            let n = buf.len().min(PREFACE.len());
            if buf[..n] != PREFACE[..n] {
                self.goaway(Code::Protocol);
                return buf.len();
            }
            if n < PREFACE.len() {
                return 0;
            }
            self.greeted = true;
            at = PREFACE.len();
        }
        while !self.done && buf.len() - at >= 9 {
            let h = &buf[at..at + 9];
            let len = (h[0] as usize) << 16 | (h[1] as usize) << 8 | h[2] as usize;
            if len > MAX_FRAME {
                self.goaway(Code::FrameSize);
                return buf.len();
            }
            if buf.len() - at - 9 < len {
                break;
            }
            let (kind, flags) = (h[3], h[4]);
            let id = u32::from_be_bytes([h[5], h[6], h[7], h[8]]) & 0x7fff_ffff;
            let payload = &buf[at + 9..at + 9 + len];
            at += 9 + len;
            match self.frame_in(kind, flags, id, payload) {
                Ok(()) => {}
                Err(Fault::Conn(code)) => {
                    self.goaway(code);
                    return buf.len();
                }
                Err(Fault::Stream(id, code)) => {
                    // Resets the client draws from us count as its own: a
                    // frame that makes us drop a started handler (a zero
                    // WINDOW_UPDATE, DATA on an ended stream) is a reset
                    // flood too ("MadeYouReset").
                    self.reset(id, code);
                    self.resets += 1;
                    if self.resets > self.answered.saturating_add(RESET_BUDGET) {
                        self.goaway(Code::Calm);
                        return buf.len();
                    }
                }
            }
        }
        if self.done {
            return buf.len();
        }
        // The windows open again, once a frame's worth of what the client
        // may rely on is spent. An update goes out after the data it
        // covers is read, so a client that has seen only the older ones can
        // never send past the counts.
        if self.recv <= WINDOW / 2 {
            self.window_update(0, (WINDOW - self.recv) as u32);
            self.recv = WINDOW;
        }
        // A stream's window opens again only while the bodies still coming
        // hold at most `MAX_HELD`, or for the oldest of them: a client
        // cannot make 100 streams each hold a body limit at once, and the
        // oldest always goes on, so none waits for good.
        let (mut held, mut oldest) = (0, u32::MAX);
        for s in self.streams.iter().filter(|s| !s.ended) {
            held += s.req.body.len();
            oldest = oldest.min(s.id);
        }
        for i in 0..self.streams.len() {
            let s = &mut self.streams[i];
            if s.recv <= WINDOW / 2 && !s.ended && (held <= MAX_HELD || s.id == oldest) {
                let (id, n) = (s.id, (WINDOW - s.recv) as u32);
                s.recv = WINDOW;
                self.window_update(id, n);
            }
        }
        at
    }

    fn frame_in(&mut self, kind: u8, flags: u8, id: u32, p: &[u8]) -> Result<(), Fault> {
        use Fault::{Conn, Stream as St};
        // A header block in pieces: nothing may come between.
        if self.block.is_some() && kind != CONTINUATION {
            return Err(Conn(Code::Protocol));
        }
        match kind {
            DATA => {
                if id == 0 {
                    return Err(Conn(Code::Protocol));
                }
                let data = unpad(flags, p)?;
                // Flow control counts the padding too.
                if p.is_empty() && flags & END_STREAM == 0 {
                    self.spend()?;
                }
                // More than the window we gave is an error (RFC 9113 6.9);
                // `feed` gives it back when the frames are read.
                self.recv -= p.len() as i64;
                if self.recv < 0 {
                    return Err(Conn(Code::FlowControl));
                }
                let Some(i) = self.find(id) else {
                    return Err(match id > self.last {
                        true => Conn(Code::Protocol),
                        false => St(id, Code::StreamClosed),
                    });
                };
                if self.streams[i].ended {
                    return Err(St(id, Code::StreamClosed));
                }
                let s = &mut self.streams[i];
                s.recv -= p.len() as i64;
                if s.recv < 0 {
                    return Err(St(id, Code::FlowControl));
                }
                let limit = self.limit;
                let s = &mut self.streams[i];
                if !s.req.too_large {
                    if s.req.body.len() + data.len() > limit(&s.req.path) {
                        s.req.too_large = true;
                        s.req.body = Vec::new();
                    } else {
                        s.req.body.extend_from_slice(data);
                    }
                }
                if flags & END_STREAM != 0 {
                    self.end(i)?;
                }
                Ok(())
            }
            HEADERS => {
                if id == 0 {
                    return Err(Conn(Code::Protocol));
                }
                let mut b = unpad(flags, p)?;
                if flags & PRIORITY_FLAG != 0 {
                    if b.len() < 5 {
                        // Padding that took the priority's room.
                        let code = match flags & PADDED {
                            0 => Code::FrameSize,
                            _ => Code::Protocol,
                        };
                        return Err(Conn(code));
                    }
                    let dep = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) & 0x7fff_ffff;
                    b = &b[5..];
                    if dep == id {
                        self.block_start(id, flags, b)?;
                        // Its CONTINUATION, still to come, must not open it.
                        if let Some(b) = &mut self.block {
                            b.reset = true;
                        }
                        return Err(St(id, Code::Protocol));
                    }
                }
                self.block_start(id, flags, b)
            }
            PRIORITY => {
                if id == 0 {
                    return Err(Conn(Code::Protocol));
                }
                if p.len() != 5 {
                    return Err(St(id, Code::FrameSize));
                }
                self.spend()?;
                let dep = u32::from_be_bytes([p[0], p[1], p[2], p[3]]) & 0x7fff_ffff;
                if dep == id {
                    return Err(St(id, Code::Protocol));
                }
                Ok(())
            }
            RST_STREAM => {
                if id == 0 {
                    return Err(Conn(Code::Protocol));
                }
                if p.len() != 4 {
                    return Err(Conn(Code::FrameSize));
                }
                if id > self.last {
                    return Err(Conn(Code::Protocol));
                }
                self.resets += 1;
                if self.resets > self.answered.saturating_add(RESET_BUDGET) {
                    return Err(Conn(Code::Calm));
                }
                self.forget(id);
                Ok(())
            }
            SETTINGS => {
                if id != 0 {
                    return Err(Conn(Code::Protocol));
                }
                self.spend()?;
                if flags & ACK != 0 {
                    return match p.is_empty() {
                        true => Ok(()),
                        false => Err(Conn(Code::FrameSize)),
                    };
                }
                if !p.len().is_multiple_of(6) {
                    return Err(Conn(Code::FrameSize));
                }
                for s in p.as_chunks::<6>().0 {
                    let v = u32::from_be_bytes([s[2], s[3], s[4], s[5]]);
                    match u16::from_be_bytes([s[0], s[1]]) {
                        2 if v > 1 => return Err(Conn(Code::Protocol)),
                        4 => {
                            if v as i64 > MAX_WINDOW {
                                return Err(Conn(Code::FlowControl));
                            }
                            let delta = v as i64 - self.peer_window;
                            self.peer_window = v as i64;
                            for st in &mut self.streams {
                                st.send += delta;
                                if st.send > MAX_WINDOW {
                                    return Err(Conn(Code::FlowControl));
                                }
                            }
                        }
                        5 => {
                            if !(16_384..=16_777_215).contains(&v) {
                                return Err(Conn(Code::Protocol));
                            }
                            // We never send more than our own maximum.
                            self.peer_frame = (v as usize).min(MAX_FRAME);
                        }
                        _ => {}
                    }
                }
                self.frame(SETTINGS, ACK, 0, &[]);
                Ok(())
            }
            PUSH_PROMISE => Err(Conn(Code::Protocol)),
            PING => {
                if id != 0 {
                    return Err(Conn(Code::Protocol));
                }
                if p.len() != 8 {
                    return Err(Conn(Code::FrameSize));
                }
                self.spend()?;
                if flags & ACK == 0 {
                    self.frame(PING, ACK, 0, p);
                }
                Ok(())
            }
            GOAWAY => {
                if id != 0 {
                    return Err(Conn(Code::Protocol));
                }
                // The client goes: what it asked is still answered.
                Ok(())
            }
            WINDOW_UPDATE => {
                if p.len() != 4 {
                    return Err(Conn(Code::FrameSize));
                }
                let n = (u32::from_be_bytes([p[0], p[1], p[2], p[3]]) & 0x7fff_ffff) as i64;
                if id == 0 {
                    if n == 0 {
                        return Err(Conn(Code::Protocol));
                    }
                    self.send += n;
                    if self.send > MAX_WINDOW {
                        return Err(Conn(Code::FlowControl));
                    }
                    return Ok(());
                }
                if n == 0 {
                    return Err(St(id, Code::Protocol));
                }
                match self.find(id) {
                    Some(i) => {
                        self.streams[i].send += n;
                        if self.streams[i].send > MAX_WINDOW {
                            return Err(St(id, Code::FlowControl));
                        }
                        Ok(())
                    }
                    None if id > self.last => Err(Conn(Code::Protocol)),
                    None => Ok(()),
                }
            }
            CONTINUATION => {
                let Some(b) = &mut self.block else {
                    return Err(Conn(Code::Protocol));
                };
                if b.id != id {
                    return Err(Conn(Code::Protocol));
                }
                b.frames += 1;
                // A flood of small pieces, or a block too large to take.
                if b.bytes.len() + p.len() > MAX_HEADER_LIST || b.frames > 64 {
                    return Err(Conn(Code::Calm));
                }
                b.bytes.extend_from_slice(p);
                if flags & END_HEADERS != 0 {
                    return self.block_end();
                }
                Ok(())
            }
            _ => self.spend(),
        }
    }

    /// One more frame that asks work for no request: too many is a flood.
    fn spend(&mut self) -> Result<(), Fault> {
        self.idle += 1;
        match self.idle > IDLE_FRAME_BUDGET {
            true => Err(Fault::Conn(Code::Calm)),
            false => Ok(()),
        }
    }

    fn block_start(&mut self, id: u32, flags: u8, b: &[u8]) -> Result<(), Fault> {
        if b.len() > MAX_HEADER_LIST {
            return Err(Fault::Conn(Code::Calm));
        }
        self.block = Some(Block {
            id,
            end_stream: flags & END_STREAM != 0,
            bytes: b.to_vec(),
            frames: 0,
            reset: false,
        });
        match flags & END_HEADERS != 0 {
            true => self.block_end(),
            false => Ok(()),
        }
    }

    /// A whole header block: a request's head, or its trailers.
    fn block_end(&mut self) -> Result<(), Fault> {
        let Some(b) = self.block.take() else {
            return Err(Fault::Conn(Code::Internal));
        };
        let mut fields = Vec::new();
        // Decoded whatever the stream's fate, to keep the table in step.
        if let Err(code) = self.hpack.decode(&b.bytes, &mut fields) {
            return Err(Fault::Conn(code));
        }
        let id = b.id;
        if b.reset {
            self.last = self.last.max(id);
            return Ok(());
        }
        if let Some(i) = self.find(id) {
            // Trailers: they must end the stream, and are dropped.
            if self.streams[i].ended {
                return Err(Fault::Stream(id, Code::StreamClosed));
            }
            if !b.end_stream {
                return Err(Fault::Conn(Code::Protocol));
            }
            if fields.iter().any(|(n, _)| n.starts_with(b":")) {
                return Err(Fault::Stream(id, Code::Protocol));
            }
            return self.end(i);
        }
        if id % 2 == 0 || id <= self.last {
            return Err(Fault::Conn(Code::Protocol));
        }
        self.last = id;
        if self.streams.len() >= MAX_STREAMS {
            return Err(Fault::Stream(id, Code::Refused));
        }
        let (req, length) = request(fields).map_err(|c| Fault::Stream(id, c))?;
        self.streams.push(Stream {
            id,
            send: self.peer_window,
            recv: WINDOW,
            ended: false,
            req,
            length,
        });
        if b.end_stream {
            let i = self.streams.len() - 1;
            self.end(i)?;
        }
        Ok(())
    }

    /// The client ended stream `i`: its request is whole.
    fn end(&mut self, i: usize) -> Result<(), Fault> {
        let s = &mut self.streams[i];
        if let Some(n) = s.length
            && !s.req.too_large
            && n != s.req.body.len() as u64
        {
            return Err(Fault::Stream(s.id, Code::Protocol));
        }
        s.ended = true;
        self.ready.push_back(s.id);
        Ok(())
    }

    fn find(&self, id: u32) -> Option<usize> {
        self.streams.iter().position(|s| s.id == id)
    }

    fn forget(&mut self, id: u32) {
        if let Some(i) = self.find(id) {
            self.streams.swap_remove(i);
        }
        self.ready.retain(|&r| r != id);
    }

    /// The request of `id`, to answer: `None` when it was reset meanwhile.
    pub(crate) fn take(&mut self, id: u32) -> Option<Request> {
        let i = self.find(id)?;
        self.idle = 0;
        Some(std::mem::take(&mut self.streams[i].req))
    }

    /// Whether stream `id` is still open for us to send on.
    pub(crate) fn open(&self, id: u32) -> bool {
        self.find(id).is_some()
    }

    /// Sends the head of the answer on `id`: `status` and `headers`
    /// (lowercase names). `end`: there is no body.
    pub(crate) fn head(&mut self, id: u32, status: u16, headers: &[(&[u8], &[u8])], end: bool) {
        let mut block = Vec::with_capacity(256);
        encode_status(&mut block, status);
        for (n, v) in headers {
            if connection_specific(n) {
                continue;
            }
            block.push(0); // literal, not indexed, new name
            encode_string(&mut block, n);
            encode_string(&mut block, v);
        }
        let flags = if end { END_STREAM } else { 0 };
        let mut first = true;
        let mut rest = &block[..];
        loop {
            let n = rest.len().min(self.peer_frame);
            let last = n == rest.len();
            let kind = if first { HEADERS } else { CONTINUATION };
            let f = if last { END_HEADERS } else { 0 } | if first { flags } else { 0 };
            self.frame(kind, f, id, &rest[..n]);
            rest = &rest[n..];
            first = false;
            if last {
                break;
            }
        }
        if end {
            self.finish(id);
        }
    }

    /// We ended stream `id`: one answered.
    fn finish(&mut self, id: u32) {
        self.answered += 1;
        self.idle = 0;
        self.forget(id);
    }

    /// Sends what of `data` the windows let through on `id`: how much.
    /// `end`: `data` is the rest of the body, so the last of it ends the
    /// stream. A reset stream takes all, and sends none.
    pub(crate) fn data(&mut self, id: u32, data: &[u8], end: bool) -> usize {
        let Some(i) = self.find(id) else {
            return data.len();
        };
        let mut at = 0;
        loop {
            let room = self.send.min(self.streams[i].send).max(0) as usize;
            let n = (data.len() - at).min(room).min(self.peer_frame);
            let last = at + n == data.len();
            if n == 0 && !(last && end) {
                return at;
            }
            let f = if last && end { END_STREAM } else { 0 };
            self.frame(DATA, f, id, &data[at..at + n]);
            self.send -= n as i64;
            self.streams[i].send -= n as i64;
            at += n;
            if last {
                if end {
                    self.finish(id);
                }
                return at;
            }
        }
    }

    /// Ends the connection with `code`, after the streams we started.
    pub(crate) fn goaway(&mut self, code: Code) {
        if self.done {
            return;
        }
        let mut p = [0; 8];
        p[..4].copy_from_slice(&self.last.to_be_bytes());
        p[4..].copy_from_slice(&(code as u32).to_be_bytes());
        self.frame(GOAWAY, 0, 0, &p);
        self.done = true;
    }

    fn reset(&mut self, id: u32, code: Code) {
        self.frame(RST_STREAM, 0, id, &(code as u32).to_be_bytes());
        self.forget(id);
    }

    fn window_update(&mut self, id: u32, n: u32) {
        self.frame(WINDOW_UPDATE, 0, id, &n.to_be_bytes());
    }

    fn frame(&mut self, kind: u8, flags: u8, id: u32, p: &[u8]) {
        let n = p.len() as u32;
        self.out.extend_from_slice(&n.to_be_bytes()[1..]);
        self.out.extend_from_slice(&[kind, flags]);
        self.out.extend_from_slice(&id.to_be_bytes());
        self.out.extend_from_slice(p);
    }
}

/// A frame's payload without its padding.
fn unpad(flags: u8, p: &[u8]) -> Result<&[u8], Fault> {
    if flags & PADDED == 0 {
        return Ok(p);
    }
    let Some((&pad, rest)) = p.split_first() else {
        return Err(Fault::Conn(Code::FrameSize));
    };
    match rest.len().checked_sub(pad as usize) {
        Some(n) => Ok(&rest[..n]),
        None => Err(Fault::Conn(Code::Protocol)),
    }
}

/// Headers HTTP/2 has no use for, and must refuse (RFC 9113 §8.2.2).
fn connection_specific(n: &[u8]) -> bool {
    [
        &b"connection"[..],
        b"keep-alive",
        b"proxy-connection",
        b"transfer-encoding",
        b"upgrade",
    ]
    .iter()
    .any(|c| n.eq_ignore_ascii_case(c))
}

/// A request from a stream's decoded header fields (RFC 9113 §8.3).
fn request(fields: Vec<(Vec<u8>, Vec<u8>)>) -> Result<(Request, Option<u64>), Code> {
    let mut r = Request::default();
    let (mut scheme, mut regular, mut length) = (false, false, None);
    let text = |v: Vec<u8>| String::from_utf8(v).map_err(|_| Code::Protocol);
    for (n, v) in fields {
        if n.iter().any(|b| b.is_ascii_uppercase()) || n.is_empty() {
            return Err(Code::Protocol);
        }
        if n[0] == b':' {
            if regular {
                return Err(Code::Protocol);
            }
            let slot = match &n[..] {
                b":method" => &mut r.method,
                b":path" => &mut r.path,
                b":authority" => &mut r.authority,
                b":scheme" => {
                    if scheme {
                        return Err(Code::Protocol);
                    }
                    scheme = true;
                    continue;
                }
                _ => return Err(Code::Protocol),
            };
            if !slot.is_empty() {
                return Err(Code::Protocol);
            }
            *slot = text(v)?;
            continue;
        }
        regular = true;
        if connection_specific(&n) || (n == b"te" && v != b"trailers") {
            return Err(Code::Protocol);
        }
        let name = text(n)?;
        // Digits only, and a repeat must agree, as on HTTP/1.
        if name == "content-length" {
            let n = crate::http::parse_decimal(&v).ok_or(Code::Protocol)? as u64;
            if length.is_some_and(|m| m != n) {
                return Err(Code::Protocol);
            }
            length = Some(n);
        }
        // Crumbs of `cookie` (RFC 9113 8.2.3) are one header to the app.
        if name == "cookie"
            && let Some((_, all)) = r.headers.iter_mut().find(|(n, _)| n == "cookie")
        {
            all.extend_from_slice(b"; ");
            all.extend_from_slice(&v);
            continue;
        }
        r.headers.push((name, v));
    }
    if r.method.is_empty() || r.path.is_empty() || !scheme || r.method == "CONNECT" {
        return Err(Code::Protocol);
    }
    Ok((r, length))
}

// ---- HPACK (RFC 7541) ----

const STATIC: [(&str, &str); 61] = [
    (":authority", ""),
    (":method", "GET"),
    (":method", "POST"),
    (":path", "/"),
    (":path", "/index.html"),
    (":scheme", "http"),
    (":scheme", "https"),
    (":status", "200"),
    (":status", "204"),
    (":status", "206"),
    (":status", "304"),
    (":status", "400"),
    (":status", "404"),
    (":status", "500"),
    ("accept-charset", ""),
    ("accept-encoding", "gzip, deflate"),
    ("accept-language", ""),
    ("accept-ranges", ""),
    ("accept", ""),
    ("access-control-allow-origin", ""),
    ("age", ""),
    ("allow", ""),
    ("authorization", ""),
    ("cache-control", ""),
    ("content-disposition", ""),
    ("content-encoding", ""),
    ("content-language", ""),
    ("content-length", ""),
    ("content-location", ""),
    ("content-range", ""),
    ("content-type", ""),
    ("cookie", ""),
    ("date", ""),
    ("etag", ""),
    ("expect", ""),
    ("expires", ""),
    ("from", ""),
    ("host", ""),
    ("if-match", ""),
    ("if-modified-since", ""),
    ("if-none-match", ""),
    ("if-range", ""),
    ("if-unmodified-since", ""),
    ("last-modified", ""),
    ("link", ""),
    ("location", ""),
    ("max-forwards", ""),
    ("proxy-authenticate", ""),
    ("proxy-authorization", ""),
    ("range", ""),
    ("referer", ""),
    ("refresh", ""),
    ("retry-after", ""),
    ("server", ""),
    ("set-cookie", ""),
    ("strict-transport-security", ""),
    ("transfer-encoding", ""),
    ("user-agent", ""),
    ("vary", ""),
    ("via", ""),
    ("www-authenticate", ""),
];

/// The HPACK decoder: its dynamic table, newest first.
struct Decoder {
    table: VecDeque<(Vec<u8>, Vec<u8>)>,
    size: usize,
    max: usize,
}

impl Decoder {
    fn new() -> Decoder {
        Decoder {
            table: VecDeque::new(),
            size: 0,
            max: TABLE_SIZE,
        }
    }

    /// Decodes a header block onto `out`. The list may not grow past
    /// `MAX_HEADER_LIST`, however few bytes ask for it (an HPACK bomb).
    fn decode(&mut self, mut b: &[u8], out: &mut Vec<(Vec<u8>, Vec<u8>)>) -> Result<(), Code> {
        let mut list = 0usize;
        let mut first = true;
        while let Some(&c) = b.first() {
            let field = if c & 0x80 != 0 {
                let i = int(&mut b, 7)?;
                self.get(i)?
            } else if c & 0xe0 == 0x20 {
                // A table size update: only at the start of a block.
                if !first {
                    return Err(Code::Compression);
                }
                let n = int(&mut b, 5)?;
                if n > TABLE_SIZE {
                    return Err(Code::Compression);
                }
                self.max = n;
                self.evict(0);
                continue;
            } else {
                let (prefix, index) = match c & 0x40 != 0 {
                    true => (6, true),
                    false => (4, false),
                };
                let i = int(&mut b, prefix)?;
                let name = match i {
                    0 => string(&mut b)?,
                    i => self.get(i)?.0,
                };
                let value = string(&mut b)?;
                if index {
                    self.insert(name.clone(), value.clone());
                }
                (name, value)
            };
            first = false;
            list += field.0.len() + field.1.len() + 32;
            if list > MAX_HEADER_LIST {
                return Err(Code::Calm);
            }
            out.push(field);
        }
        Ok(())
    }

    fn get(&self, i: usize) -> Result<(Vec<u8>, Vec<u8>), Code> {
        if i == 0 {
            return Err(Code::Compression);
        }
        if let Some((n, v)) = STATIC.get(i - 1) {
            return Ok((n.as_bytes().to_vec(), v.as_bytes().to_vec()));
        }
        match self.table.get(i - 1 - STATIC.len()) {
            Some((n, v)) => Ok((n.clone(), v.clone())),
            None => Err(Code::Compression),
        }
    }

    fn insert(&mut self, name: Vec<u8>, value: Vec<u8>) {
        let size = name.len() + value.len() + 32;
        self.evict(size);
        // Larger than the table: it empties it, and is not kept.
        if size <= self.max {
            self.size += size;
            self.table.push_front((name, value));
        }
    }

    /// Drops the oldest entries until `room` more fits.
    fn evict(&mut self, room: usize) {
        while self.size + room > self.max {
            let Some((n, v)) = self.table.pop_back() else {
                self.size = 0;
                return;
            };
            self.size -= n.len() + v.len() + 32;
        }
    }
}

/// An HPACK integer with a `prefix`-bit start, which it consumes.
fn int(b: &mut &[u8], prefix: u8) -> Result<usize, Code> {
    let Some((&first, mut rest)) = b.split_first() else {
        return Err(Code::Compression);
    };
    let max = (1usize << prefix) - 1;
    let mut n = first as usize & max;
    if n == max {
        let mut shift = 0;
        loop {
            let Some((&c, r)) = rest.split_first() else {
                return Err(Code::Compression);
            };
            rest = r;
            // Past 2^28 is no length or index we could take.
            if shift > 21 {
                return Err(Code::Compression);
            }
            n += ((c & 0x7f) as usize) << shift;
            shift += 7;
            if c & 0x80 == 0 {
                break;
            }
        }
    }
    *b = rest;
    Ok(n)
}

/// An HPACK string literal, which it consumes.
fn string(b: &mut &[u8]) -> Result<Vec<u8>, Code> {
    let huffman = b.first().is_some_and(|&c| c & 0x80 != 0);
    let n = int(b, 7)?;
    if n > b.len() || n > MAX_HEADER_LIST {
        return Err(Code::Compression);
    }
    let (s, rest) = b.split_at(n);
    *b = rest;
    match huffman {
        true => huffman_decode(s),
        false => Ok(s.to_vec()),
    }
}

fn encode_int(out: &mut Vec<u8>, first: u8, prefix: u8, mut n: usize) {
    let max = (1usize << prefix) - 1;
    if n < max {
        out.push(first | n as u8);
        return;
    }
    out.push(first | max as u8);
    n -= max;
    while n >= 128 {
        out.push((n % 128) as u8 | 0x80);
        n /= 128;
    }
    out.push(n as u8);
}

fn encode_string(out: &mut Vec<u8>, s: &[u8]) {
    encode_int(out, 0, 7, s.len());
    out.extend_from_slice(s);
}

fn encode_status(out: &mut Vec<u8>, status: u16) {
    let i = match status {
        200 => 8,
        204 => 9,
        206 => 10,
        304 => 11,
        400 => 12,
        404 => 13,
        500 => 14,
        _ => {
            out.push(0x08); // literal, not indexed, name `:status`
            let mut d = [b'0'; 3];
            let s = status.min(999);
            d[0] += (s / 100) as u8;
            d[1] += (s / 10 % 10) as u8;
            d[2] += (s % 10) as u8;
            encode_string(out, &d);
            return;
        }
    };
    out.push(0x80 | i);
}

/// The bit length of each symbol's Huffman code (RFC 7541 Appendix B);
/// the code is canonical, so the lengths are all of it. 256 is EOS.
const HUFFMAN_LEN: [u8; 257] = [
    13, 23, 28, 28, 28, 28, 28, 28, 28, 24, 30, 28, 28, 30, 28, 28, 28, 28, 28, 28, 28, 28, 30, 28,
    28, 28, 28, 28, 28, 28, 28, 28, 6, 10, 10, 12, 13, 6, 8, 11, 10, 10, 8, 11, 8, 6, 6, 6, 5, 5,
    5, 6, 6, 6, 6, 6, 6, 6, 7, 8, 15, 6, 12, 10, 13, 6, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 8, 7, 8, 13, 19, 13, 14, 6, 15, 5, 6, 5, 6, 5, 6, 6, 6, 5, 7, 7, 6, 6,
    6, 5, 6, 7, 6, 5, 5, 6, 7, 7, 7, 7, 7, 15, 11, 14, 13, 28, 20, 22, 20, 20, 22, 22, 22, 23, 22,
    23, 23, 23, 23, 23, 24, 23, 24, 24, 22, 23, 24, 23, 23, 23, 23, 21, 22, 23, 22, 23, 23, 24, 22,
    21, 20, 22, 22, 23, 23, 21, 23, 22, 22, 24, 21, 22, 23, 23, 21, 21, 22, 21, 23, 22, 23, 23, 20,
    22, 22, 22, 23, 22, 22, 23, 26, 26, 20, 19, 22, 23, 22, 25, 26, 26, 26, 27, 27, 26, 24, 25, 19,
    21, 26, 27, 27, 26, 27, 24, 21, 21, 26, 26, 28, 27, 27, 27, 20, 24, 20, 21, 22, 21, 21, 23, 22,
    22, 25, 25, 24, 24, 26, 23, 26, 27, 26, 26, 27, 27, 27, 27, 27, 28, 27, 27, 27, 27, 27, 26, 30,
];

/// The canonical code's tables: the symbols by (length, symbol), how many
/// codes each length has, and the first code of each.
struct Canon {
    symbols: [u16; 257],
    count: [u16; 31],
    first: [u32; 31],
    start: [u16; 31],
}

const CANON: Canon = {
    let mut c = Canon {
        symbols: [0; 257],
        count: [0; 31],
        first: [0; 31],
        start: [0; 31],
    };
    let mut s = 0;
    while s < 257 {
        c.count[HUFFMAN_LEN[s] as usize] += 1;
        s += 1;
    }
    let (mut code, mut at, mut len) = (0u32, 0u16, 1);
    while len <= 30 {
        code = (code + c.count[len - 1] as u32) << 1;
        c.first[len] = code;
        c.start[len] = at;
        let mut s = 0;
        while s < 257 {
            if HUFFMAN_LEN[s] as usize == len {
                c.symbols[at as usize] = s as u16;
                at += 1;
            }
            s += 1;
        }
        len += 1;
    }
    c
};

/// Decodes a Huffman string: padding of up to 7 one bits ends it, and the
/// EOS symbol may not appear (RFC 7541 §5.2).
fn huffman_decode(s: &[u8]) -> Result<Vec<u8>, Code> {
    let mut out = Vec::with_capacity(s.len() * 8 / 5);
    let (mut code, mut len, mut ones) = (0u32, 0usize, true);
    for &byte in s {
        for bit in (0..8).rev() {
            let b = (byte >> bit) as u32 & 1;
            code = code << 1 | b;
            len += 1;
            ones &= b == 1;
            let offset = code.wrapping_sub(CANON.first[len]);
            if offset < CANON.count[len] as u32 {
                let sym = CANON.symbols[(CANON.start[len] as u32 + offset) as usize];
                if sym == 256 {
                    return Err(Code::Compression);
                }
                out.push(sym as u8);
                (code, len, ones) = (0, 0, true);
            } else if len == 30 {
                return Err(Code::Compression);
            }
        }
    }
    match len <= 7 && ones {
        true => Ok(out),
        false => Err(Code::Compression),
    }
}

// ---- The connection ----

#[cfg(not(target_arch = "wasm32"))]
pub(super) use driver::serve;

#[cfg(not(target_arch = "wasm32"))]
mod driver {
    use super::{Request, Session};
    use crate::App;
    use crate::cx::Cx;
    use crate::cx::Method;
    use crate::http::{
        Buffers, Conn, body_limit, decide, give_buffers, read, seconds, serialize_h2, take_buffers,
    };
    use crate::policy;
    use std::future::{Future, poll_fn};
    use std::io;
    use std::net::SocketAddr;
    use std::pin::{Pin, pin};
    use std::task::{Context, Poll};
    use tokio::sync::mpsc::Receiver;

    /// What a stream's handler made: the head of its answer and the body.
    struct Prepared {
        status: u16,
        /// Lowercase names.
        headers: Vec<(Vec<u8>, Vec<u8>)>,
        /// The body, or its start when `body` streams the rest.
        rest: Vec<u8>,
        body: Option<Receiver<Vec<u8>>>,
    }

    /// A stream being answered: its handler running, then its body going.
    enum Phase<F> {
        Deciding(Pin<Box<F>>),
        /// `bytes[at..]` waits for the windows; `body` brings more, and
        /// when it is `None` the last of `bytes` ends the stream.
        Sending {
            bytes: Vec<u8>,
            at: usize,
            body: Option<Receiver<Vec<u8>>>,
        },
    }

    struct Job<F> {
        id: u32,
        phase: Phase<F>,
    }

    /// Serves an HTTP/2 connection whose first bytes (the preface, maybe
    /// more) are in `b.cx.wire.buf`, until it closes. Each whole request
    /// becomes a [`Job`] polled beside the read, so one long answer holds
    /// back no other stream.
    pub(in crate::http) async fn serve<A: App>(
        stream: &mut Conn,
        b: &mut Buffers,
        mut timer: Pin<&mut tokio::time::Sleep>,
    ) {
        let mut s = Session::new(body_limit::<A>);
        let mut inbuf = std::mem::take(&mut b.cx.wire.buf);
        let peer = b.cx.wire.peer;
        let mut jobs = Vec::new();
        loop {
            let used = s.feed(&inbuf);
            inbuf.drain(..used);
            while let Some(id) = s.ready.pop_front() {
                if s.done {
                    break;
                }
                let Some(req) = s.take(id) else { continue };
                if req.too_large {
                    s.head(id, 413, &[], true);
                    continue;
                }
                let phase = Phase::Deciding(Box::pin(prepare::<A>(req, peer)));
                jobs.push(Job { id, phase });
            }
            // A reset stream's job goes: its handler or body is dropped.
            jobs.retain(|j| s.open(j.id));
            pump(&mut s, &mut jobs);
            if !s.out.is_empty() && stream.write(&mut s.out).await.is_err() {
                return;
            }
            if s.done || jobs.is_empty() && crate::http::stopping() {
                stream.shutdown().await;
                return;
            }
            inbuf.reserve(super::MAX_FRAME + 9);
            // Only the windows hold the answers back (or there are none):
            // the client owes us frames, so it has the idle deadline.
            let owed = jobs.iter().all(|j| match &j.phase {
                Phase::Deciding(_) => false,
                Phase::Sending { bytes, at, body } => *at < bytes.len() || body.is_none(),
            });
            let event = match owed {
                true => {
                    let deadline = policy::idle_deadline(seconds());
                    let rd = read(stream, &mut inbuf, timer.as_mut(), deadline);
                    wait(rd, &mut s, &mut jobs).await
                }
                false => wait(stream.read(&mut inbuf), &mut s, &mut jobs).await,
            };
            match event {
                None | Some(Ok(1..)) => {}
                Some(_) => return,
            }
        }
    }

    /// Waits for frames from the client (`Some`, what the read gave) or for
    /// a job to move (`None`). The read is safe to drop unfinished.
    async fn wait<F: Future<Output = Prepared>>(
        rd: impl Future<Output = io::Result<usize>>,
        s: &mut Session,
        jobs: &mut [Job<F>],
    ) -> Option<io::Result<usize>> {
        let mut rd = pin!(rd);
        poll_fn(|cx| {
            if let Poll::Ready(r) = rd.as_mut().poll(cx) {
                return Poll::Ready(Some(r));
            }
            match progress(s, jobs, cx) {
                true => Poll::Ready(None),
                false => Poll::Pending,
            }
        })
        .await
    }

    /// Polls each job once: whether one moved (a head ready, a chunk come,
    /// a body ended).
    fn progress<F: Future<Output = Prepared>>(
        s: &mut Session,
        jobs: &mut [Job<F>],
        cx: &mut Context,
    ) -> bool {
        let mut moved = false;
        for j in jobs.iter_mut() {
            match &mut j.phase {
                Phase::Deciding(f) => {
                    let Poll::Ready(p) = f.as_mut().poll(cx) else {
                        continue;
                    };
                    moved = true;
                    let list: Vec<(&[u8], &[u8])> =
                        p.headers.iter().map(|(n, v)| (&n[..], &v[..])).collect();
                    let end = p.rest.is_empty() && p.body.is_none();
                    s.head(j.id, p.status, &list, end);
                    j.phase = Phase::Sending {
                        bytes: p.rest,
                        at: 0,
                        body: p.body,
                    };
                }
                Phase::Sending { bytes, at, body } => {
                    let Some(rx) = body else { continue };
                    if *at < bytes.len() {
                        continue;
                    }
                    match rx.poll_recv(cx) {
                        Poll::Ready(Some(chunk)) => {
                            *bytes = chunk;
                            *at = 0;
                        }
                        Poll::Ready(None) => *body = None,
                        Poll::Pending => continue,
                    }
                    moved = true;
                }
            }
        }
        moved
    }

    /// Sends what the windows let through, a frame per stream in turn, so
    /// the answers in flight share the connection. Ended jobs go.
    fn pump<F>(s: &mut Session, jobs: &mut Vec<Job<F>>) {
        loop {
            let mut moved = false;
            for j in jobs.iter_mut() {
                let Phase::Sending { bytes, at, body } = &mut j.phase else {
                    continue;
                };
                let end = body.is_none();
                if *at == bytes.len() && !end {
                    continue;
                }
                let n = (bytes.len() - *at).min(s.peer_frame);
                let whole = *at + n == bytes.len();
                let sent = s.data(j.id, &bytes[*at..*at + n], end && whole);
                *at += sent;
                moved |= sent > 0 || !s.open(j.id);
                if *at == bytes.len() && !end {
                    // The chunk went: its memory goes with it.
                    *bytes = Vec::new();
                    *at = 0;
                }
            }
            jobs.retain(|j| s.open(j.id));
            if !moved {
                return;
            }
        }
    }

    /// Answers `req` through the HTTP/1 path, in buffers of its own, so the
    /// streams in flight never share one.
    async fn prepare<A: App>(req: Request, peer: SocketAddr) -> Prepared {
        let host = req.authority.as_bytes();
        let headers = req
            .headers
            .iter()
            .map(|(n, v)| (n.as_str(), &v[..]))
            .chain((!host.is_empty()).then_some(("host", host)));
        let mut cx = match Cx::from_request::<A>(&req.method, &req.path, headers, &req.body, peer) {
            Ok(cx) => cx,
            Err(status) => {
                return Prepared {
                    status,
                    headers: Vec::new(),
                    rest: Vec::new(),
                    body: None,
                };
            }
        };
        let mut b = take_buffers(peer);
        let Buffers {
            wbuf, out, reply, ..
        } = &mut *b;
        decide::<A>(&mut cx, out, reply, None).await;
        let head_only = cx.method == Method::Head;
        wbuf.clear();
        let (mut status, mut headers) = (500, Vec::new());
        let body = serialize_h2::<A>(wbuf, reply, out, head_only, |n, v| match n {
            ":status" => status = v.parse().unwrap_or(500),
            _ => headers.push((n.to_ascii_lowercase().into_bytes(), v.as_bytes().to_vec())),
        });
        let p = Prepared {
            status,
            headers,
            rest: wbuf.to_vec(),
            body,
        };
        give_buffers(b);
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fuzz::{Rng, mutate};

    fn hex(s: &str) -> Vec<u8> {
        let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    fn limit(_: &str) -> usize {
        1024
    }

    fn frame(kind: u8, flags: u8, id: u32, p: &[u8]) -> Vec<u8> {
        let mut s = Session::new(limit);
        s.out.clear();
        s.frame(kind, flags, id, p);
        s.out
    }

    /// A greeted session and the frames it sends after the given input.
    type Frame = (u8, u8, u32, Vec<u8>);

    fn run(input: &[Vec<u8>]) -> (Session, Vec<Frame>) {
        let mut s = Session::new(limit);
        s.out.clear();
        let mut all = PREFACE.to_vec();
        for f in input {
            all.extend_from_slice(f);
        }
        let used = s.feed(&all);
        assert!(used <= all.len());
        let mut frames = Vec::new();
        let mut b = &s.out[..];
        while b.len() >= 9 {
            let len = (b[0] as usize) << 16 | (b[1] as usize) << 8 | b[2] as usize;
            let id = u32::from_be_bytes([b[5], b[6], b[7], b[8]]);
            frames.push((b[3], b[4], id, b[9..9 + len].to_vec()));
            b = &b[9 + len..];
        }
        (s, frames)
    }

    fn goaway_code(f: &[Frame]) -> Option<u32> {
        f.iter()
            .find(|f| f.0 == GOAWAY)
            .map(|f| u32::from_be_bytes([f.3[4], f.3[5], f.3[6], f.3[7]]))
    }

    fn rst_code(f: &[Frame]) -> Option<u32> {
        f.iter()
            .find(|f| f.0 == RST_STREAM)
            .map(|f| u32::from_be_bytes([f.3[0], f.3[1], f.3[2], f.3[3]]))
    }

    fn get(path: &str) -> Vec<u8> {
        let mut b = vec![0x82, 0x86]; // :method GET, :scheme http
        b.push(0x04); // :path, literal not indexed
        encode_string(&mut b, path.as_bytes());
        b
    }

    #[test]
    fn huffman_rfc_examples() {
        for (h, s) in [
            ("f1e3 c2e5 f23a 6ba0 ab90 f4ff", "www.example.com"),
            ("a8eb 1064 9cbf", "no-cache"),
            ("25a8 49e9 5ba9 7d7f", "custom-key"),
            ("25a8 49e9 5bb8 e8b4 bf", "custom-value"),
            ("6402", "302"),
            ("aec3 771a 4b", "private"),
            (
                "d07a be94 1054 d444 a820 0595 040b 8166 e082 a62d 1bff",
                "Mon, 21 Oct 2013 20:13:21 GMT",
            ),
            (
                "9d29 ad17 1863 c78f 0b97 c8e9 ae82 ae43 d3",
                "https://www.example.com",
            ),
            ("9bd9 ab", "gzip"),
            (
                "94e7 821d d7f2 e6c7 b335 dfdf cd5b 3960 d5af 2708 7f36 72c1 ab27 0fb5 291f 9587 3160 65c0 03ed 4ee5 b106 3d50 07",
                "foo=ASDJKHQKBZXOQWEOPIUAXQWEOIU; max-age=3600; version=1",
            ),
        ] {
            assert_eq!(huffman_decode(&hex(h)).unwrap(), s.as_bytes(), "{s}");
        }
        // Padding of zeros, or longer than 7 bits, is an error.
        assert!(huffman_decode(&[0x00]).is_err());
        assert!(huffman_decode(&[0xff, 0xff, 0xff, 0xff]).is_err());
    }

    #[test]
    fn huffman_code_is_complete() {
        // Kraft: a complete prefix code sums to exactly one.
        let sum: u64 = HUFFMAN_LEN.iter().map(|&l| 1u64 << (30 - l)).sum();
        assert_eq!(sum, 1 << 30);
    }

    #[test]
    fn hpack_rfc_c3_requests_with_table() {
        // RFC 7541 C.3: three requests sharing a dynamic table.
        let mut d = Decoder::new();
        let mut out = Vec::new();
        d.decode(
            &hex("8286 8441 0f77 7777 2e65 7861 6d70 6c65 2e63 6f6d"),
            &mut out,
        )
        .unwrap();
        assert_eq!(
            out[3],
            (b":authority".to_vec(), b"www.example.com".to_vec())
        );
        out.clear();
        d.decode(&hex("8286 84be 5808 6e6f 2d63 6163 6865"), &mut out)
            .unwrap();
        assert_eq!(out[3].1, b"www.example.com");
        assert_eq!(out[4], (b"cache-control".to_vec(), b"no-cache".to_vec()));
        out.clear();
        d.decode(
            &hex("8287 85bf 400a 6375 7374 6f6d 2d6b 6579 0c63 7573 746f 6d2d 7661 6c75 65"),
            &mut out,
        )
        .unwrap();
        assert_eq!(out[2].1, b"/index.html");
        assert_eq!(out[4], (b"custom-key".to_vec(), b"custom-value".to_vec()));
        assert_eq!(d.size, 164);
    }

    #[test]
    fn hpack_rfc_c4_huffman() {
        let mut d = Decoder::new();
        let mut out = Vec::new();
        d.decode(&hex("8286 8441 8cf1 e3c2 e5f2 3a6b a0ab 90f4 ff"), &mut out)
            .unwrap();
        assert_eq!(out[3].1, b"www.example.com");
    }

    #[test]
    fn hpack_indexed_names_past_31() {
        // A literal with indexing whose name index sets bit 5 (`user-agent`,
        // 58) is no table size update; nor is one without indexing (`via`).
        let mut d = Decoder::new();
        let mut out = Vec::new();
        let mut b = vec![0x7a];
        encode_string(&mut b, b"curl");
        b.extend_from_slice(&[0x0f, 0x2d]);
        encode_string(&mut b, b"proxy");
        d.decode(&b, &mut out).unwrap();
        assert_eq!(out[0], (b"user-agent".to_vec(), b"curl".to_vec()));
        assert_eq!(out[1], (b"via".to_vec(), b"proxy".to_vec()));
        // A size update after a field is an error.
        assert!(d.decode(&[0x82, 0x20], &mut Vec::new()).is_err());
    }

    #[test]
    fn hpack_bomb_is_refused() {
        // One large entry, then a block of one-byte references to it.
        let mut d = Decoder::new();
        let mut block = vec![0x40];
        encode_string(&mut block, b"x");
        encode_string(&mut block, &[b'a'; 4000]);
        block.extend(std::iter::repeat_n(0x80 | 62, 1000));
        assert_eq!(d.decode(&block, &mut Vec::new()), Err(Code::Calm));
    }

    #[test]
    fn hpack_bad_input_never_panics() {
        let mut rng = Rng::new(7);
        for _ in 0..20_000 {
            let mut b = hex("8286 8441 8cf1 e3c2 e5f2 3a6b a0ab 90f4 ff 5808 6e6f 2d63 6163 6865");
            mutate(&mut rng, &mut b);
            let _ = Decoder::new().decode(&b, &mut Vec::new());
        }
        // Integers past what any length could be.
        let _ = Decoder::new().decode(
            &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f],
            &mut Vec::new(),
        );
        assert!(int(&mut &[0x1f, 0xff, 0xff, 0xff, 0xff, 0xff][..], 5).is_err());
    }

    #[test]
    fn frames_fuzzed_never_panic() {
        let mut rng = Rng::new(11);
        let mut seed = Vec::new();
        seed.extend(frame(SETTINGS, 0, 0, &[0, 4, 0, 0, 0xff, 0xff]));
        seed.extend(frame(HEADERS, END_HEADERS, 1, &get("/a")));
        seed.extend(frame(DATA, END_STREAM, 1, b"hi"));
        seed.extend(frame(PING, 0, 0, &[1; 8]));
        seed.extend(frame(WINDOW_UPDATE, 0, 0, &[0, 0, 1, 0]));
        seed.extend(frame(RST_STREAM, 0, 1, &[0, 0, 0, 8]));
        for _ in 0..20_000 {
            let mut b = seed.clone();
            mutate(&mut rng, &mut b);
            let (mut s, _) = run(&[b]);
            while let Some(id) = s.ready.pop_front() {
                if s.take(id).is_some() {
                    s.head(id, 200, &[(b"content-type", b"text/plain")], false);
                    s.data(id, &[b'x'; 70_000], true);
                }
            }
        }
    }

    #[test]
    fn a_get_becomes_a_request() {
        let (mut s, f) = run(&[
            frame(SETTINGS, 0, 0, &[]),
            frame(HEADERS, END_HEADERS | END_STREAM, 1, &get("/user/0")),
        ]);
        assert!(f.iter().any(|f| f.0 == SETTINGS && f.1 == ACK));
        assert_eq!(s.ready.pop_front(), Some(1));
        let r = s.take(1).unwrap();
        assert_eq!((r.method.as_str(), r.path.as_str()), ("GET", "/user/0"));
        s.out.clear();
        s.head(1, 200, &[(b"content-length", b"2")], false);
        assert_eq!(s.data(1, b"ok", true), 2);
        assert!(!s.open(1));
    }

    /// RFC 9113 8.2.3: a client may split `cookie` into crumbs, one field
    /// each; the app reads them as one header.
    #[test]
    fn cookie_crumbs_are_one_header() {
        let mut b = get("/");
        for c in ["a=1", "b=2"] {
            b.push(0x0f); // literal not indexed, name index 32 (`cookie`)
            b.push(32 - 15);
            encode_string(&mut b, c.as_bytes());
        }
        let (mut s, _) = run(&[
            frame(SETTINGS, 0, 0, &[]),
            frame(HEADERS, END_HEADERS | END_STREAM, 1, &b),
        ]);
        let r = s.take(1).unwrap();
        let cookies: Vec<_> = r.headers.iter().filter(|(n, _)| n == "cookie").collect();
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].1, b"a=1; b=2");
    }

    /// A POST on stream 3 and `n` DATA frames of 16384 bytes after it, read
    /// in one pass.
    fn upload(n: usize) -> (Session, Vec<Frame>) {
        let mut h = get("/x");
        h[0] = 0x83; // POST
        let mut input = vec![frame(HEADERS, END_HEADERS, 3, &h)];
        input.extend((0..n).map(|_| frame(DATA, 0, 3, &[0; 16384])));
        run(&input)
    }

    #[test]
    fn data_past_the_receive_window_is_a_flow_control_error() {
        // 4 frames are 65536 bytes: one more than the window we gave.
        let (_, f) = upload(4);
        assert_eq!(goaway_code(&f), Some(Code::FlowControl as u32));
        // The window, spent, is given back; and 3 frames are fine.
        let (_, f) = upload(3);
        assert_eq!(goaway_code(&f), None);
        let back = |id| {
            f.iter()
                .filter(|f| f.0 == WINDOW_UPDATE && f.2 == id)
                .count()
        };
        assert_eq!((back(0), back(3)), (1, 1));
    }

    #[test]
    fn a_stream_past_its_window_is_reset() {
        // Streams 3 and 5 share the connection's window; the 4th frame on
        // stream 3 is past its own, with the connection's to spare.
        let mut h = get("/x");
        h[0] = 0x83;
        let mut input = vec![frame(HEADERS, END_HEADERS, 3, &h)];
        input.extend((0..3).map(|_| frame(DATA, 0, 3, &[0; 16384])));
        let (mut s, f) = run(&input);
        assert_eq!(goaway_code(&f), None);
        s.recv = WINDOW;
        s.streams[0].recv = 100;
        s.out.clear();
        let mut all = frame(DATA, 0, 3, &[0; 101]);
        all.extend(frame(PING, 0, 0, &[0; 8]));
        assert_eq!(s.feed(&all), all.len());
        assert!(s.out.windows(9).any(|w| w[3] == RST_STREAM && w[8] == 3));
    }

    #[test]
    fn a_post_body_and_flow_control() {
        let mut h = get("/x");
        h[0] = 0x83; // POST
        let (mut s, f) = run(&[
            frame(HEADERS, END_HEADERS, 3, &h),
            frame(DATA, 0, 3, b"ab"),
            frame(DATA, END_STREAM | PADDED, 3, &[2, b'c', 0, 0]),
        ]);
        assert!(
            f.iter().all(|f| f.0 != WINDOW_UPDATE),
            "a little data is not given back yet"
        );
        assert_eq!(s.take(3).unwrap().body, b"abc");
        // The send window: 65535, then it waits for an update.
        s.out.clear();
        assert_eq!(s.data(3, &vec![0; 70_000], true), 65_535);
        let mut more = frame(WINDOW_UPDATE, 0, 0, &[0, 1, 0, 0]);
        more.extend(frame(WINDOW_UPDATE, 0, 3, &[0, 1, 0, 0]));
        assert_eq!(s.feed(&more), more.len());
        assert_eq!(s.data(3, &vec![0; 70_000 - 65_535], true), 70_000 - 65_535);
    }

    // h2spec-style conformance: each case and the error it must draw.
    #[test]
    fn conformance_connection_errors() {
        let cases: Vec<(&str, Vec<Vec<u8>>, Code)> = vec![
            (
                "settings on a stream",
                vec![frame(SETTINGS, 0, 1, &[])],
                Code::Protocol,
            ),
            (
                "settings ack with payload",
                vec![frame(SETTINGS, ACK, 0, &[0; 6])],
                Code::FrameSize,
            ),
            (
                "settings length",
                vec![frame(SETTINGS, 0, 0, &[0; 5])],
                Code::FrameSize,
            ),
            (
                "enable_push 2",
                vec![frame(SETTINGS, 0, 0, &[0, 2, 0, 0, 0, 2])],
                Code::Protocol,
            ),
            (
                "window too large",
                vec![frame(SETTINGS, 0, 0, &[0, 4, 0x80, 0, 0, 0])],
                Code::FlowControl,
            ),
            (
                "frame size too small",
                vec![frame(SETTINGS, 0, 0, &[0, 5, 0, 0, 0x10, 0])],
                Code::Protocol,
            ),
            (
                "ping length",
                vec![frame(PING, 0, 0, &[0; 7])],
                Code::FrameSize,
            ),
            (
                "ping on a stream",
                vec![frame(PING, 0, 1, &[0; 8])],
                Code::Protocol,
            ),
            (
                "headers on stream 0",
                vec![frame(HEADERS, END_HEADERS, 0, &get("/"))],
                Code::Protocol,
            ),
            (
                "even stream",
                vec![frame(HEADERS, END_HEADERS, 2, &get("/"))],
                Code::Protocol,
            ),
            (
                "stream ids go down",
                vec![
                    frame(HEADERS, END_HEADERS | END_STREAM, 5, &get("/")),
                    frame(HEADERS, END_HEADERS | END_STREAM, 3, &get("/")),
                ],
                Code::Protocol,
            ),
            (
                "continuation alone",
                vec![frame(CONTINUATION, END_HEADERS, 1, &[])],
                Code::Protocol,
            ),
            (
                "frame inside a header block",
                vec![frame(HEADERS, 0, 1, &get("/")), frame(PING, 0, 0, &[0; 8])],
                Code::Protocol,
            ),
            (
                "data on idle stream",
                vec![frame(DATA, 0, 1, b"x")],
                Code::Protocol,
            ),
            (
                "bad padding",
                vec![frame(DATA, PADDED, 1, &[9, 1])],
                Code::Protocol,
            ),
            (
                "push promise",
                vec![frame(PUSH_PROMISE, 0, 1, &[0; 4])],
                Code::Protocol,
            ),
            (
                "window update 0",
                vec![frame(WINDOW_UPDATE, 0, 0, &[0; 4])],
                Code::Protocol,
            ),
            (
                "window overflow",
                vec![frame(WINDOW_UPDATE, 0, 0, &[0x7f, 0xff, 0xff, 0xff])],
                Code::FlowControl,
            ),
            (
                "rst on idle",
                vec![frame(RST_STREAM, 0, 1, &[0; 4])],
                Code::Protocol,
            ),
            (
                "hpack index 0",
                vec![frame(HEADERS, END_HEADERS, 1, &[0x80])],
                Code::Compression,
            ),
            (
                "frame too large",
                vec![{
                    let mut f = frame(DATA, 0, 1, &[]);
                    f[1] = 0x40; // 16384 + 1
                    f[2] = 0x01;
                    f
                }],
                Code::FrameSize,
            ),
        ];
        for (name, input, code) in cases {
            let (_, f) = run(&input);
            assert_eq!(goaway_code(&f), Some(code as u32), "{name}");
        }
        // A wrong preface.
        let mut s = Session::new(limit);
        s.feed(b"GET / HTTP/1.1\r\n\r\n12345678");
        assert!(s.done);
    }

    #[test]
    fn conformance_stream_errors() {
        let header = |fields: &[(&[u8], &[u8])]| {
            let mut b = Vec::new();
            for (n, v) in fields {
                b.push(0);
                encode_string(&mut b, n);
                encode_string(&mut b, v);
            }
            frame(HEADERS, END_HEADERS | END_STREAM, 1, &b)
        };
        let base: [(&[u8], &[u8]); 3] = [
            (b":method", b"GET"),
            (b":scheme", b"http"),
            (b":path", b"/"),
        ];
        let with = |extra: &[(&'static [u8], &'static [u8])], first: bool| {
            let mut v: Vec<(&[u8], &[u8])> = Vec::new();
            if first {
                v.extend_from_slice(extra);
            }
            v.extend_from_slice(&base);
            if !first {
                v.extend_from_slice(extra);
            }
            header(&v)
        };
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("uppercase name", with(&[(b"X-A", b"1")], false)),
            (
                "pseudo after regular",
                with(&[(b"x-a", b"1"), (b":authority", b"a")], false),
            ),
            ("unknown pseudo", with(&[(b":foo", b"1")], true)),
            (
                "connection header",
                with(&[(b"connection", b"close")], false),
            ),
            ("te not trailers", with(&[(b"te", b"gzip")], false)),
            (
                "missing path",
                header(&[(b":method", b"GET"), (b":scheme", b"http")]),
            ),
            ("two paths", with(&[(b":path", b"/b")], true)),
            (
                "content-length mismatch",
                with(&[(b"content-length", b"3")], false),
            ),
        ];
        for (name, input) in cases {
            let (s, f) = run(&[input]);
            assert_eq!(rst_code(&f), Some(Code::Protocol as u32), "{name}");
            assert!(!s.done && s.ready.is_empty(), "{name}");
        }
        // Priority on itself.
        let (_, f) = run(&[frame(PRIORITY, 0, 1, &[0, 0, 0, 1, 16])]);
        assert_eq!(rst_code(&f), Some(Code::Protocol as u32));
        // HEADERS that depend on themselves, in pieces: the stream is reset,
        // and its CONTINUATION does not open it after all.
        let mut p = vec![0, 0, 0, 1, 16];
        p.extend(get("/"));
        let (s, f) = run(&[
            frame(HEADERS, END_STREAM | PRIORITY_FLAG, 1, &p),
            frame(CONTINUATION, END_HEADERS, 1, &[]),
        ]);
        assert_eq!(rst_code(&f), Some(Code::Protocol as u32));
        assert!(!s.done && s.ready.is_empty() && !s.open(1));
        // Data after the stream ended.
        let (_, f) = run(&[
            frame(HEADERS, END_HEADERS | END_STREAM, 1, &get("/")),
            frame(DATA, 0, 1, b"x"),
        ]);
        assert_eq!(rst_code(&f), Some(Code::StreamClosed as u32));
    }

    #[test]
    fn floods_end_the_connection() {
        // Rapid reset: open and reset, never waiting for an answer.
        let mut input = Vec::new();
        for i in 0..400u32 {
            let id = 2 * i + 1;
            input.push(frame(HEADERS, END_HEADERS, id, &get("/")));
            input.push(frame(RST_STREAM, 0, id, &[0, 0, 0, 8]));
        }
        let (_, f) = run(&input);
        assert_eq!(goaway_code(&f), Some(Code::Calm as u32));
        // CONTINUATION flood: endless small pieces.
        let mut input = vec![frame(HEADERS, 0, 1, &get("/"))];
        for _ in 0..100 {
            input.push(frame(CONTINUATION, 0, 1, &[0x40, 1, b'a', 1, b'b']));
        }
        let (_, f) = run(&input);
        assert_eq!(goaway_code(&f), Some(Code::Calm as u32));
        // A ping flood.
        let input: Vec<_> = (0..2000).map(|_| frame(PING, 0, 0, &[0; 8])).collect();
        let (_, f) = run(&input);
        assert_eq!(goaway_code(&f), Some(Code::Calm as u32));
        // Too many streams at once: refused, the connection stays.
        let input: Vec<_> = (0..101u32)
            .map(|i| frame(HEADERS, END_HEADERS, 2 * i + 1, &get("/")))
            .collect();
        let (s, f) = run(&input);
        assert_eq!(rst_code(&f), Some(Code::Refused as u32));
        assert!(!s.done);
    }

    #[test]
    fn resets_we_are_made_to_send_spend_the_budget() {
        // MadeYouReset: each stream's handler is started (taken), then a
        // zero WINDOW_UPDATE makes us reset it. It ends the connection as a
        // client's own resets would.
        let mut s = Session::new(limit);
        s.feed(PREFACE);
        for i in 0..400u32 {
            let id = 2 * i + 1;
            s.feed(&frame(HEADERS, END_HEADERS | END_STREAM, id, &get("/")));
            assert!(s.take(id).is_some() || s.done);
            s.feed(&frame(WINDOW_UPDATE, 0, id, &[0; 4]));
        }
        assert!(s.done, "a flood of resets we sent ends the connection");
        // A few, as a client that errs now and then makes, do not.
        let mut s = Session::new(limit);
        s.feed(PREFACE);
        for i in 0..50u32 {
            let id = 2 * i + 1;
            s.feed(&frame(HEADERS, END_HEADERS | END_STREAM, id, &get("/")));
            s.take(id);
            s.feed(&frame(WINDOW_UPDATE, 0, id, &[0; 4]));
        }
        assert!(!s.done);
    }

    #[test]
    fn many_uploads_at_once_hold_little() {
        // Thirty streams each send three frames: past `MAX_HELD`, only the
        // oldest stream's window opens again.
        fn big(_: &str) -> usize {
            1 << 30
        }
        let mut s = Session::new(big);
        s.feed(PREFACE);
        let mut h = get("/x");
        h[0] = 0x83; // POST
        let mut heads = Vec::new();
        for i in 0..30u32 {
            heads.extend(frame(HEADERS, END_HEADERS, 2 * i + 1, &h));
        }
        s.feed(&heads);
        s.out.clear();
        for i in 0..30u32 {
            let mut one = Vec::new();
            for _ in 0..3 {
                one.extend(frame(DATA, 0, 2 * i + 1, &[0; 16384]));
            }
            s.recv = WINDOW;
            s.feed(&one);
        }
        let opened: Vec<u32> = (0..s.out.len().saturating_sub(8))
            .filter(|&k| s.out[k + 3] == WINDOW_UPDATE && s.out[k..k + 3] == [0, 0, 4])
            .map(|k| u32::from_be_bytes([s.out[k + 5], s.out[k + 6], s.out[k + 7], s.out[k + 8]]))
            .filter(|&id| id != 0)
            .collect();
        assert!(opened.contains(&1), "the oldest goes on: {opened:?}");
        assert!(opened.len() < 30, "not every stream: {opened:?}");
        // Once the streams before it end, the first held back goes on.
        let next = (0..30u32)
            .map(|i| 2 * i + 1)
            .find(|id| !opened.contains(id));
        let next = next.expect("one held back") as u8;
        let mut w = Vec::new();
        for &id in &opened {
            w.extend(frame(DATA, END_STREAM, id, &[]));
        }
        s.out.clear();
        s.feed(&w);
        assert!(
            s.out
                .windows(9)
                .any(|f| f[3] == WINDOW_UPDATE && f[8] == next)
        );
    }

    #[test]
    fn content_length_is_digits_and_agrees() {
        let with = |cl: &[&[u8]]| {
            let mut h = get("/x");
            h[0] = 0x83;
            for v in cl {
                h.push(0);
                encode_string(&mut h, b"content-length");
                encode_string(&mut h, v);
            }
            let (_, f) = run(&[frame(HEADERS, END_HEADERS, 1, &h)]);
            rst_code(&f)
        };
        assert_eq!(with(&[b"3"]), None);
        assert_eq!(with(&[b"3", b"3"]), None);
        assert_eq!(with(&[b"3", b"4"]), Some(Code::Protocol as u32));
        assert_eq!(with(&[b"+3"]), Some(Code::Protocol as u32));
        assert_eq!(with(&[b" 3"]), Some(Code::Protocol as u32));
    }

    #[test]
    fn large_bodies_and_heads() {
        let mut h = get("/x");
        h[0] = 0x83;
        let (mut s, _) = run(&[
            frame(HEADERS, END_HEADERS, 1, &h),
            frame(DATA, END_STREAM, 1, &[0; 2000]),
        ]);
        assert!(s.take(1).unwrap().too_large);
        // A head larger than a frame goes out in CONTINUATION frames.
        let (mut s, _) = run(&[frame(HEADERS, END_HEADERS | END_STREAM, 1, &get("/"))]);
        s.take(1);
        s.out.clear();
        let v = vec![b'v'; 20_000];
        s.head(1, 299, &[(b"x-big", &v)], true);
        assert_eq!(s.out[3], HEADERS);
        let first = (s.out[1] as usize) << 8 | s.out[2] as usize;
        assert_eq!(first, MAX_FRAME);
        assert_eq!(s.out[9 + first + 3], CONTINUATION);
    }

    #[test]
    fn a_stalled_stream_holds_back_no_other() {
        let (mut s, _) = run(&[
            frame(HEADERS, END_HEADERS | END_STREAM, 1, &get("/big")),
            frame(HEADERS, END_HEADERS | END_STREAM, 3, &get("/small")),
        ]);
        s.take(1);
        s.take(3);
        s.head(1, 200, &[], false);
        s.head(3, 200, &[], false);
        // Stream 1 shrinks to a 10-byte window; the connection's is open.
        let mut w = frame(SETTINGS, 0, 0, &[0, 4, 0, 0, 0, 10]);
        w.extend(frame(WINDOW_UPDATE, 0, 3, &[0, 0, 0xff, 0xf5]));
        assert_eq!(s.feed(&w), w.len());
        assert_eq!(s.data(1, &[1; 100], false), 10, "stalled at its window");
        assert_eq!(s.data(1, &[1; 90], false), 0);
        assert_eq!(s.data(3, b"done", true), 4, "the other stream goes on");
        assert!(!s.open(3));
        // An update resumes it.
        let w = frame(WINDOW_UPDATE, 0, 1, &[0, 0, 0, 90]);
        assert_eq!(s.feed(&w), w.len());
        assert_eq!(s.data(1, &[1; 90], true), 90);
        assert!(!s.open(1));
    }

    #[test]
    fn a_reset_ends_a_streaming_answer() {
        let (mut s, _) = run(&[frame(HEADERS, END_HEADERS | END_STREAM, 1, &get("/events"))]);
        s.take(1);
        s.head(1, 200, &[], false);
        assert_eq!(s.data(1, b"data: 1\n\n", false), 9);
        let r = frame(RST_STREAM, 0, 1, &[0, 0, 0, 8]);
        assert_eq!(s.feed(&r), r.len());
        assert!(!s.open(1), "the driver drops its body");
        s.out.clear();
        assert_eq!(s.data(1, b"data: 2\n\n", false), 9, "taken, not sent");
        assert!(s.out.is_empty());
        assert!(!s.done);
        // Handlers started and then reset never earn reset budget.
        let mut s = Session::new(limit);
        s.feed(PREFACE);
        for i in 0..400u32 {
            let id = 2 * i + 1;
            s.feed(&frame(HEADERS, END_HEADERS | END_STREAM, id, &get("/")));
            assert!(s.take(id).is_some() || s.done);
            s.feed(&frame(RST_STREAM, 0, id, &[0, 0, 0, 8]));
        }
        assert!(s.done, "a reset flood of taken streams ends the connection");
    }
}
