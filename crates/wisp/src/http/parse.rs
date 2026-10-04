//! Parsing: the request head and chunked bodies.

use super::*;

/// Parses the request starting at `cx.wire.buf[at..]` and, if it is complete,
/// records it in `cx` as spans. `on_wire`: from a client of the built-in
/// server, where HTTP/1.1 must name its host (RFC 9112 §3.2); a request
/// from another host (`Cx::from_request`) may have none.
pub(super) fn parse<A: App>(cx: &mut Cx, at: usize, on_wire: bool) -> Parsed {
    cx.wire.headers.clear();
    let mut seen = Seen::default();
    let head = match fast_head(&cx.wire.buf, at, &mut cx.wire.headers, &mut seen) {
        Some(head) => head,
        None => {
            cx.wire.headers.clear();
            match slow_head(&cx.wire.buf, at, &mut cx.wire.headers) {
                Ok(head) => {
                    seen = Seen::of(&cx.wire.buf, &cx.wire.headers);
                    head
                }
                Err(parsed) => return parsed,
            }
        }
    };
    if seen.refuse != 0 {
        return Parsed::Invalid(seen.refuse);
    }
    let keep_alive = seen.keep.unwrap_or(head.http11);
    // Never to HTTP/1.0, which has no such answer (RFC 9110 §10.1.1).
    let expect_continue = head.http11 && seen.expect;
    let (content_length, hosts) = (seen.length, seen.hosts);
    cx.wire.knows = seen.knows;
    cx.wire.known = seen.known;
    let chunked = seen.chunked > 0;
    // Both framings at once is the classic smuggling vector; HTTP/1.0 has
    // no chunked framing at all. Two hosts could be read as either, by
    // Wisp and a proxy or cache in front of it (RFC 9112 §3.2).
    if (chunked && (content_length.is_some() || seen.chunked > 1 || !head.http11)) || hosts > 1 {
        return Parsed::Invalid(400);
    }

    cx.method = head.method;
    cx.wire.http11 = head.http11;
    cx.wire.path = head.path;
    // A const: without a base path (`WISP_BASE`) this is no code at all.
    if !crate::protocol::BASE.is_empty() {
        cx.wire.path = under_base(&cx.wire.buf, head.path);
    }
    cx.wire.query = head.query;
    // A body's limit is its route's: the request is routed for it now, once.
    let body_start = at + head.len;
    let len = content_length.unwrap_or(0);
    let mut routed = None;
    let mut limit = 0;
    if chunked || len > 0 {
        let route = cx.path().starts_with('/').then(|| route::<A>(cx));
        limit = limit_of::<A>(route.flatten());
        routed = route;
    }
    let buf = &cx.wire.buf[..];
    let (len, total) = if chunked {
        match chunks(&buf[body_start..], limit) {
            Chunks::Complete { wire, body } => (body, head.len + wire),
            Chunks::Partial => {
                return Parsed::Partial {
                    need: 0,
                    expect_continue,
                    body: true,
                };
            }
            Chunks::TooLarge => return Parsed::Invalid(413),
            Chunks::Invalid => return Parsed::Invalid(400),
        }
    } else {
        if len > limit {
            return Parsed::Invalid(413);
        }
        if buf.len() - body_start < len {
            return Parsed::Partial {
                need: head.len + len,
                expect_continue,
                body: true,
            };
        }
        (len, head.len + len)
    };

    // Once whole, as the limits come first.
    if on_wire && hosts == 0 && head.http11 {
        return Parsed::Invalid(400);
    }
    cx.wire.body = Span {
        start: body_start as u32,
        len: len as u32,
    };
    if chunked {
        // The body's data moves up over the chunk framing, into one piece
        // where `cx.wire.body` says. The head stays as it is.
        unchunk(&mut cx.wire.buf[body_start..at + total]);
    }
    Parsed::Request(Req {
        len: total,
        keep_alive,
        routed,
    })
}

/// A request's head: its line, and its headers, which went to `cx.wire.headers`.
pub(super) struct Head {
    /// Its bytes, up to the body.
    pub(super) len: usize,
    pub(super) method: Method,
    pub(super) path: Span,
    pub(super) query: Span,
    /// HTTP/1.1 rather than 1.0.
    pub(super) http11: bool,
}

/// What the headers say that the framing and the connection need, each
/// read once: as [`fast_head`] passes it, or after [`slow_head`].
#[derive(Default, PartialEq, Debug)]
pub(super) struct Seen {
    pub(super) length: Option<usize>,
    /// `transfer-encoding: chunked` headers.
    pub(super) chunked: u32,
    pub(super) hosts: u32,
    /// What `connection` said last: keep-alive or close.
    pub(super) keep: Option<bool>,
    /// The last `expect` is `100-continue`.
    pub(super) expect: bool,
    pub(super) knows: u8,
    pub(super) known: [Span; KNOWN],
    /// The answer to the first header refused, or 0.
    pub(super) refuse: u16,
}

impl Seen {
    pub(super) fn of(buf: &[u8], headers: &[(Span, Span)]) -> Seen {
        let mut seen = Seen::default();
        for &(name, value) in headers {
            let name = header_name(buf, name.start as usize, name.len as usize);
            seen.header(buf, name, value);
        }
        seen
    }

    #[inline(always)]
    pub(super) fn header(&mut self, buf: &[u8], name: Name, span: Span) {
        let value = &buf[span.range()];
        match name {
            Name::Other => {}
            Name::Host => self.hosts += 1,
            // Strict: digits only, and repeated headers must agree (smuggling).
            Name::ContentLength => match (parse_decimal(value), self.length) {
                (Some(n), None) => self.length = Some(n),
                (Some(n), Some(m)) if n == m => {}
                _ => self.refuse(400),
            },
            // Only `chunked`, alone: gzip and the like are for a proxy.
            Name::TransferEncoding if swar::eq_lower(value.trim_ascii(), b"chunked") => {
                self.chunked += 1
            }
            Name::TransferEncoding => self.refuse(501),
            // One token, the usual case, before splitting the list.
            Name::Connection => match value.trim_ascii() {
                v if swar::eq_lower(v, b"keep-alive") => self.keep = Some(true),
                v if swar::eq_lower(v, b"close") => self.keep = Some(false),
                v => {
                    for token in v.split(|&b| b == b',').map(<[u8]>::trim_ascii) {
                        if swar::eq_lower(token, b"close") {
                            self.keep = Some(false);
                        } else if swar::eq_lower(token, b"keep-alive") {
                            self.keep = Some(true);
                        }
                    }
                }
            },
            Name::Expect => self.expect = swar::eq_lower(value.trim_ascii(), b"100-continue"),
            Name::Known(k) => {
                if self.knows & 1 << k as u8 == 0 {
                    self.knows |= 1 << k as u8;
                    self.known[k as usize] = span;
                }
            }
        }
    }

    pub(super) fn refuse(&mut self, code: u16) {
        if self.refuse == 0 {
            self.refuse = code;
        }
    }
}

/// The headers the parser acts on, and the [`Known`] ones it marks for `Cx`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Name {
    Host,
    ContentLength,
    TransferEncoding,
    Connection,
    Expect,
    Known(Known),
    Other,
}

/// The eight bytes of `s` from `from`, zero past its end, as [`swar::word`]
/// reads them.
pub(super) const fn key(s: &[u8], from: usize) -> u64 {
    let (mut x, mut k) = (0, 0);
    while k < 8 && from + k < s.len() {
        x |= (s[from + k] as u64) << (8 * k);
        k += 1;
    }
    x
}

/// The first and last eight bytes of `s`, at least eight long.
pub(super) const fn ends(s: &[u8]) -> (u64, u64) {
    (key(s, 0), key(s, s.len() - 8))
}

/// Which header the name `b[at..at + len]` is, in any case. It is a token
/// (`tchar`s), so setting bit 5 of every byte lowercases its letters and
/// keeps `-`, and turns no other `tchar` into either: its length and two
/// words (three past 16 bytes) against the lowercase names tell.
#[inline(always)]
pub(crate) fn header_name(b: &[u8], at: usize, len: usize) -> Name {
    const LOWER: u64 = 0x2020_2020_2020_2020;
    if len < 8 {
        if len == 0 {
            return Name::Other;
        }
        let w = if at + 8 <= b.len() {
            swar::word(b, at)
        } else {
            swar::tail(&b[at..at + len], 0)
        };
        // Past the name, zeros, as in the keys: a key is one length's.
        const HOST: u64 = key(b"host", 0);
        const EXPECT: u64 = key(b"expect", 0);
        const ACCEPT: u64 = key(b"accept", 0);
        const ORIGIN: u64 = key(b"origin", 0);
        return match (w | LOWER) & u64::MAX >> (8 * (8 - len)) {
            HOST => Name::Host,
            EXPECT => Name::Expect,
            ACCEPT => Name::Known(Known::Accept),
            ORIGIN => Name::Known(Known::Origin),
            _ => Name::Other,
        };
    }
    let word = |i: usize| swar::word(b, i) | LOWER;
    let w = (word(at), word(at + len - 8));
    match len {
        10 if w == const { ends(b"connection") } => Name::Connection,
        12 if w == const { ends(b"content-type") } => Name::Known(Known::ContentType),
        12 if w == const { ends(crate::protocol::HEADER_ERROR.as_bytes()) } => {
            Name::Known(Known::WispError)
        }
        13 if w == const { ends(b"if-none-match") } => Name::Known(Known::IfNoneMatch),
        14 if w == const { ends(b"content-length") } => Name::ContentLength,
        14 if w == const { ends(b"sec-fetch-site") } => Name::Known(Known::SecFetchSite),
        17 if w == const { ends(b"transfer-encoding") }
            && word(at + 8) == const { key(b"transfer-encoding", 8) } =>
        {
            Name::TransferEncoding
        }
        _ => Name::Other,
    }
}

/// The bytes of a header name (RFC 9110 `tchar`), as httparse takes them.
pub(super) const TOKEN: [bool; 256] = {
    let mut t = [false; 256];
    let mut c = 0;
    while c < 256 {
        t[c] = (c as u8).is_ascii_alphanumeric();
        c += 1;
    }
    let symbols = b"!#$%&'*+-.^_`|~";
    let mut k = 0;
    while k < symbols.len() {
        t[symbols[k] as usize] = true;
        k += 1;
    }
    t
};

/// `HTTP/1.1` and `HTTP/1.0` as [`swar::word`] reads them.
pub(super) const V11: u64 = u64::from_le_bytes(*b"HTTP/1.1");
pub(super) const V10: u64 = u64::from_le_bytes(*b"HTTP/1.0");

/// The first byte of `b[i..]` that is not visible ASCII, or is `also`,
/// eight bytes at a time; `b.len()` if none.
#[inline(always)]
pub(super) fn visible(b: &[u8], mut i: usize, also: u8) -> usize {
    while i + 8 <= b.len() {
        let x = swar::word(b, i);
        let stop = swar::below(x, 0x21) | swar::above(x, 0x7e) | swar::eq(x, also);
        if stop != 0 {
            return i + swar::first(stop);
        }
        i += 8;
    }
    while b
        .get(i)
        .is_some_and(|&c| (0x21..=0x7e).contains(&c) && c != also)
    {
        i += 1;
    }
    i
}

/// A few bytes, at most eight, to match a word against: `key` as
/// [`swar::word`] reads it, in its first `mask` bytes, letters in any case.
pub(super) struct Pat {
    pub(super) key: u64,
    pub(super) case: u64,
    pub(super) mask: u64,
}

impl Pat {
    pub(super) const fn of(s: &[u8]) -> Pat {
        let (mut key, mut case, mut k) = (0, 0, 0);
        while k < s.len() {
            key |= (s[k] as u64) << (8 * k);
            if s[k].is_ascii_lowercase() {
                case |= 0x20 << (8 * k);
            }
            k += 1;
        }
        Pat {
            key,
            case,
            mask: u64::MAX >> (8 * (8 - s.len())),
        }
    }

    /// Setting bit 5 of a letter's byte makes either case its lowercase,
    /// and nothing else that lowercase: other bytes must be the same.
    #[inline(always)]
    pub(super) fn is(&self, x: u64) -> bool {
        (x | self.case) & self.mask == self.key
    }
}

/// The length and kind of the name at `b[i..]` when it is one of the headers every
/// request sends, with its colon after it (as a match of its bytes, it is
/// a `tchar` name): `host`, `connection`, `content-length`, `user-agent`.
/// 0 for others, whose bytes are then checked.
#[inline(always)]
pub(super) fn by_name(b: &[u8], i: usize) -> (usize, Name) {
    const HOST: Pat = Pat::of(b"host:");
    const CONTENT: Pat = Pat::of(b"content-");
    const LENGTH: Pat = Pat::of(b"length:");
    const CONNECTI: Pat = Pat::of(b"connecti");
    const ON: Pat = Pat::of(b"on:");
    const USER_AGE: Pat = Pat::of(b"user-age");
    const NT: Pat = Pat::of(b"nt:");
    let Some(w) = b.get(i..i + 16) else {
        return (0, Name::Other);
    };
    let (x, y) = (swar::word(w, 0), swar::word(w, 8));
    match x as u8 | 0x20 {
        b'h' if HOST.is(x) => (4, Name::Host),
        b'c' if CONTENT.is(x) && LENGTH.is(y) => (14, Name::ContentLength),
        b'c' if CONNECTI.is(x) && ON.is(y) => (10, Name::Connection),
        b'u' if USER_AGE.is(x) && NT.is(y) => (10, Name::Other),
        _ => (0, Name::Other),
    }
}

/// The head of `buf[at..]` when it has the usual shape: a method in
/// capitals, a target of visible ASCII, `HTTP/1.1` or `HTTP/1.0`, lines that
/// end in CRLF, header names of `tchar`s, and all of it here, in at most
/// `MAX_HEAD` bytes. Header values are scanned 16 bytes at a time. It is
/// `None` for anything else, which [`slow_head`] (httparse) then reads,
/// or refuses: what this reads, httparse reads the same.
pub(super) fn fast_head(
    buf: &[u8],
    at: usize,
    headers: &mut Vec<(Span, Span)>,
    seen: &mut Seen,
) -> Option<Head> {
    let b = &buf[..buf.len().min(at + MAX_HEAD)];
    let mut i = at;
    let method = if b.get(i..i + 4) == Some(b"GET ") {
        i += 3;
        Method::Get
    } else if b.get(i..i + 5) == Some(b"POST ") {
        i += 4;
        Method::Post
    } else {
        while b.get(i).is_some_and(u8::is_ascii_uppercase) {
            i += 1;
        }
        Method::parse(&b[at..i])
    };
    if i == at || b.get(i) != Some(&b' ') {
        return None;
    }
    i += 1;

    // The target, to the first byte that is not visible ASCII: its space.
    // Its path ends at its first `?`, where its query starts.
    let target = i;
    let path_end = visible(b, target, b'?');
    let end = match b.get(path_end) {
        Some(b'?') => visible(b, path_end + 1, b' '),
        _ => path_end,
    };
    let line = b.get(end..end + 11)?;
    let http11 = match swar::word(line, 1) {
        V11 => true,
        V10 => false,
        _ => return None,
    };
    // Empty (its space), or not a path: `*`, `http://host/x`.
    if line[0] != b' ' || line[9..] != *b"\r\n" || b[target] != b'/' {
        return None;
    }
    i = end + 11;
    let span = |from: usize, to: usize| Span {
        start: from as u32,
        len: (to - from) as u32,
    };
    let (path, query) = if path_end == end {
        (span(target, end), Span::default())
    } else {
        (span(target, path_end), span(path_end + 1, end))
    };

    loop {
        if b.get(i..i + 2)? == b"\r\n" {
            return Some(Head {
                len: i + 2 - at,
                method,
                path,
                query,
                http11,
            });
        }
        // The name: one read most by its bytes, or else letters, digits
        // and `-` eight at a time, any other `tchar` one at a time.
        let name = i;
        let (n, mut kind) = by_name(b, i);
        i += n;
        if i == name {
            loop {
                if i + 8 <= b.len() {
                    let miss = swar::not_name(swar::word(b, i));
                    if miss == 0 {
                        i += 8;
                        continue;
                    }
                    i += swar::first(miss);
                }
                if !b.get(i).is_some_and(|&c| TOKEN[usize::from(c)]) {
                    break;
                }
                i += 1;
            }
            if i == name || b.get(i) != Some(&b':') {
                return None;
            }
            kind = header_name(b, name, i - name);
        }
        let name = span(name, i);
        // Its colon, and the space after it most send.
        i += if b.get(i + 1) == Some(&b' ') { 2 } else { 1 };
        while b.get(i).is_some_and(|&c| c == b' ' || c == b'\t') {
            i += 1;
        }
        // The value, to its CR: past visible ASCII, spaces, tabs and
        // bytes of 0x80 and up; any other control stops it.
        let value = i;
        loop {
            if i + 8 <= b.len() {
                let stop = swar::control(swar::word(b, i));
                if stop == 0 {
                    i += 8;
                    // Past sixteen, a long value (a cookie, a user agent):
                    // sixteen at a time.
                    if i - value >= 16 {
                        while let Some(chunk) = b.get(i..i + 16)
                            && !swar::any_control(chunk)
                        {
                            i += 16;
                        }
                    }
                    continue;
                }
                i += swar::first(stop);
            } else {
                while b.get(i).is_some_and(|&c| !swar::is_control(c)) {
                    i += 1;
                }
            }
            if b.get(i) != Some(&b'\t') {
                break;
            }
            i += 1;
        }
        if b.get(i..i + 2)? != b"\r\n" {
            return None;
        }
        let mut value_end = i;
        while value_end > value && matches!(b[value_end - 1], b' ' | b'\t') {
            value_end -= 1;
        }
        if headers.len() == MAX_HEADERS {
            return None;
        }
        let value = span(value, value_end);
        headers.push((name, value));
        seen.header(b, kind, value);
        i += 2;
    }
}

/// The head of `buf[at..]` by httparse, which takes what [`fast_head`] does
/// not: bare LF line ends, empty lines before the request, other methods
/// and targets. `Err` is what to answer: wait for more, or refuse it.
pub(super) fn slow_head(
    buf: &[u8],
    at: usize,
    headers: &mut Vec<(Span, Span)>,
) -> Result<Head, Parsed> {
    // Left uninitialized: zeroing them cost more than parsing a small request.
    let mut raw = [const { MaybeUninit::uninit() }; MAX_HEADERS];
    let mut req = httparse::Request::new(&mut []);
    let len = match req.parse_with_uninit_headers(&buf[at..], &mut raw) {
        Ok(httparse::Status::Complete(n)) if n <= MAX_HEAD => n,
        Ok(httparse::Status::Partial) if buf.len() - at <= MAX_HEAD => {
            return Err(Parsed::Partial {
                need: 0,
                expect_continue: false,
                body: false,
            });
        }
        Ok(_) | Err(httparse::Error::TooManyHeaders) => return Err(Parsed::Invalid(431)),
        Err(_) => return Err(Parsed::Invalid(400)),
    };
    let target = req.path.unwrap_or("").as_bytes();
    let (mut path, query) = match target.iter().position(|&c| c == b'?') {
        Some(q) => (&target[..q], &target[q + 1..]),
        None => (target, &b""[..]),
    };
    // The absolute form a proxy sends, `http://host/x` (RFC 9112 §3.2.2): its
    // path, and its host in place of the `host` header's.
    let mut host = None;
    if let Some((authority, origin)) = absolute_form(path) {
        (host, path) = (Some(Span::of(buf, authority)), origin);
    }
    for h in req.headers.iter() {
        let value = match host {
            Some(host) if h.name.eq_ignore_ascii_case("host") => host,
            _ => Span::of(buf, h.value),
        };
        headers.push((Span::of(buf, h.name.as_bytes()), value));
    }
    Ok(Head {
        len,
        method: Method::parse(req.method.unwrap_or("").as_bytes()),
        path: Span::of(buf, path),
        query: Span::of(buf, query),
        http11: req.version == Some(1),
    })
}

/// The authority and path of an absolute-form target's path part
/// (`http://host:80/x`): `http` or `https`, a host without user info. The
/// path of `http://host` is `/`, the slash before `host`.
pub(super) fn absolute_form(t: &[u8]) -> Option<(&[u8], &[u8])> {
    if t.first() == Some(&b'/') {
        return None; // the usual origin form
    }
    let colon = t.iter().position(|&c| c == b':')?;
    let scheme = &t[..colon];
    if !(scheme.eq_ignore_ascii_case(b"http") || scheme.eq_ignore_ascii_case(b"https"))
        || t.get(colon + 1..colon + 3) != Some(b"//")
    {
        return None;
    }
    let host = colon + 3;
    let end = t[host..]
        .iter()
        .position(|&c| c == b'/')
        .map_or(t.len(), |i| host + i);
    if end == host || t[host..end].contains(&b'@') {
        return None;
    }
    let path = if end < t.len() {
        &t[end..]
    } else {
        &t[host - 1..host]
    };
    Some((&t[host..end], path))
}

/// The largest body any route takes, whatever its limit says: requests are
/// described by 32-bit offsets into the read buffer.
pub(super) const MAX_BODY: usize = 1 << 31;

/// The body limit for a request to `path`: its route's `BODY_LIMIT`, or
/// `WISP_BODY_LIMIT`, and at most `MAX_BODY`. Only requests with a body
/// look it up.
pub(crate) fn body_limit<A: App>(path: &str) -> usize {
    let route = path.starts_with('/').then(|| find::<A>(path)).flatten();
    limit_of::<A>(route.map(|(id, _)| id))
}

/// [`body_limit`] of a request routed to `route`.
pub(super) fn limit_of<A: App>(route: Option<usize>) -> usize {
    let usual = crate::settings().body_limit;
    route
        .and_then(|id| A::ROUTES[id].limit(usual))
        .unwrap_or(usual)
        .min(MAX_BODY)
}

pub(super) enum Chunks {
    /// The body is `body` bytes, framed in `wire` bytes.
    Complete {
        wire: usize,
        body: usize,
    },
    Partial,
    TooLarge,
    Invalid,
}

/// Reads a chunked body (RFC 9112 §7.1) from the start of `b`, without
/// changing it. Strict: hex sizes of at most 16 digits, CRLF line ends,
/// extensions and trailers skipped but bounded, so that framing overhead
/// cannot make a body far larger on the wire than `limit`.
pub(super) fn chunks(b: &[u8], limit: usize) -> Chunks {
    // A line ending in CRLF from `from`: the index of its CR.
    let line = |from: usize| -> Result<Option<usize>, ()> {
        for (i, &c) in b.iter().enumerate().skip(from).take(MAX_HEAD) {
            match c {
                b'\r' if b.get(i + 1) == Some(&b'\n') => return Ok(Some(i)),
                b'\r' if i + 1 == b.len() => return Ok(None),
                b'\r' | b'\n' => return Err(()),
                _ => {}
            }
        }
        if b.len() - from.min(b.len()) > MAX_HEAD {
            Err(())
        } else {
            Ok(None)
        }
    };
    let (mut i, mut body) = (0usize, 0usize);
    loop {
        let end = match line(i) {
            Ok(Some(end)) => end,
            Ok(None) => return Chunks::Partial,
            Err(()) => return Chunks::Invalid,
        };
        let digits = b[i..end]
            .iter()
            .take_while(|c| c.is_ascii_hexdigit())
            .count();
        let rest = &b[i + digits..end];
        if digits == 0
            || digits > 16
            || !(rest.is_empty() || rest.trim_ascii_start().starts_with(b";"))
        {
            return Chunks::Invalid;
        }
        let Ok(size) = usize::try_from(parse_hex(&b[i..i + digits])) else {
            return Chunks::TooLarge;
        };
        i = end + 2;
        if size == 0 {
            // Trailer fields, up to an empty line.
            let trailers = i;
            loop {
                match line(i) {
                    Ok(Some(end)) if end == i => return Chunks::Complete { wire: i + 2, body },
                    Ok(Some(end)) => i = end + 2,
                    Ok(None) => return Chunks::Partial,
                    Err(()) => return Chunks::Invalid,
                }
                if i - trailers > MAX_HEAD {
                    return Chunks::Invalid;
                }
            }
        }
        body = match body.checked_add(size) {
            Some(n) if n <= limit => n,
            _ => return Chunks::TooLarge,
        };
        // Framing may at most double the body, plus a little.
        if i > body.saturating_mul(2).saturating_add(MAX_HEAD) {
            return Chunks::TooLarge;
        }
        if b.len() - i < size + 2 {
            return Chunks::Partial;
        }
        if &b[i + size..i + size + 2] != b"\r\n" {
            return Chunks::Invalid;
        }
        i += size + 2;
    }
}

/// Moves the data of the chunked body `b`, which [`chunks`] found complete,
/// to its start.
pub(super) fn unchunk(b: &mut [u8]) {
    let (mut i, mut w) = (0, 0);
    loop {
        let digits = b[i..].iter().take_while(|c| c.is_ascii_hexdigit()).count();
        let size = parse_hex(&b[i..i + digits]) as usize;
        i += b[i..].windows(2).position(|p| p == b"\r\n").unwrap_or(0) + 2;
        if size == 0 {
            return;
        }
        b.copy_within(i..i + size, w);
        w += size;
        i += size + 2;
    }
}

/// At most 16 hex digits, so it fits.
pub(super) fn parse_hex(digits: &[u8]) -> u64 {
    digits
        .iter()
        .fold(0, |n, &c| n << 4 | u64::from(hex_digit(c).unwrap_or(0)))
}

thread_local! {
    /// The body of the last response this thread wrote, emptied, for the
    /// next one to fill: a warm server allocates no bodies.
    static SPARE: Cell<Vec<u8>> = const { Cell::new(Vec::new()) };
}

/// An empty buffer for a response body, with the room of an earlier one.
pub(crate) fn spare() -> Vec<u8> {
    SPARE.take()
}

/// [`spare`] for `len` bytes that may be kept long (a `String` input a
/// handler stores): only one about that size, else a new buffer, so a
/// small value never holds a large one's room.
pub(crate) fn spare_for(len: usize) -> Vec<u8> {
    let b = SPARE.take();
    if (len..=len.max(16) * 4).contains(&b.capacity()) {
        return b;
    }
    SPARE.set(b);
    Vec::with_capacity(len)
}

/// Keeps `body`, now written, for [`spare`] to hand out again, unless one
/// large message made it big.
pub(super) fn recycle(mut body: Vec<u8>) {
    if policy::kept(body.capacity()) {
        body.clear();
        SPARE.set(body);
    }
}
