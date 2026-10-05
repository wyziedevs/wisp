//! Routing: the base path, a request's route and params, trailing slashes.

use super::*;

/// `path` (a span of `buf`) without the base path: the same when it is not
/// under it. `/app` alone is `/app/`'s, `/`: the one `/` of the request
/// line that is not in the target, the one of `HTTP/1.1`.
#[cold]
#[inline(never)]
pub(super) fn under_base(buf: &[u8], path: Span) -> Span {
    under(crate::protocol::BASE.as_bytes(), buf, path)
}

pub(super) fn under(base: &[u8], buf: &[u8], path: Span) -> Span {
    let bytes = &buf[path.range()];
    match bytes.strip_prefix(base) {
        Some(rest) if rest.first() == Some(&b'/') => Span {
            start: path.start + base.len() as u32,
            len: path.len - base.len() as u32,
        },
        Some([]) => buf[path.range().end..]
            .iter()
            .position(|&b| b == b'/')
            .map_or(path, |i| Span {
                start: (path.range().end + i) as u32,
                len: 1,
            }),
        _ => path,
    }
}

/// The route of the request in `cx`, which gets its params: the matched
/// parameters as spans, so `cx` can be handed out mutably.
pub(super) fn route<A: App>(cx: &mut Cx) -> Option<usize> {
    let (id, raw) = find::<A>(cx.path())?;
    // Only the route's own: most have none.
    let names = A::ROUTES[id].params;
    if names.is_empty() {
        cx.clear_params();
        return Some(id);
    }
    let mut params = [Span::default(); crate::cx::MAX_PARAMS];
    for k in 0..names.len() {
        // `reroute` may hand back a path of its own: no parameter to give.
        if A::REROUTE && !Span::within(&cx.wire.buf, raw[k].as_bytes()) {
            return None;
        }
        params[k] = Span::of(&cx.wire.buf, raw[k].as_bytes());
    }
    cx.set_params(names, &params[..names.len()]);
    Some(id)
}

/// The answer of an edge warm-up instance (`edge::warming`): the request
/// routed and a page's headers set, by no app code (no `reroute`, hook or
/// handler), so V8 has compiled that much before the first real request.
#[cfg(target_arch = "wasm32")]
pub(crate) fn warm<A: App>(cx: &mut Cx) -> Reply {
    let _ = A::route(cx.path());
    crate::headers::page(cx);
    let mut reply = Reply::default();
    reply.set_plain(404, "");
    // The page's shell around an empty body: the runtime's, no template.
    reply.body = Body::Bytes(page::<A>(&mut Out::default()).concat().into_bytes());
    cx.send_headers(&mut reply.headers);
    reply
}

/// The route of `path`, and its parameters. A path that ends in `/` is
/// its route's without it too, when [`trailing_slash`] serves it: looked
/// for only when the path itself matches nothing.
#[inline(always)]
pub(super) fn find<A: App>(path: &str) -> Option<(usize, [&str; crate::cx::MAX_PARAMS])> {
    // A const: without `reroute` in `hooks.rs`, the path as it is.
    let path = if A::REROUTE { A::reroute(path) } else { path };
    match A::route(path) {
        Some(found) => Some(found),
        None => find_missing::<A>(path),
    }
}

/// [`find`] of a path no route matches: a rewrite's route (not for a file
/// of the app's, served as it is), else the route of its slashless form.
#[cold]
#[inline(never)]
pub(super) fn find_missing<A: App>(path: &str) -> Option<(usize, [&str; crate::cx::MAX_PARAMS])> {
    let rewritten = A::REWRITES.then(|| A::rewrite(path)).flatten();
    match rewritten.filter(|_| A::asset(path).is_none()) {
        Some(found) => Some(found),
        None => find_slash::<A>(path),
    }
}

#[cold]
#[inline(never)]
pub(super) fn find_slash<A: App>(path: &str) -> Option<(usize, [&str; crate::cx::MAX_PARAMS])> {
    match path.len() > 1 && path.ends_with('/') && slash() != TrailingSlash::Never {
        true => A::route(&path[..path.len() - 1]),
        false => None,
    }
}

/// How a page's address ends, which `wisp::trailing_slash` sets in
/// `init`. The other form gets a 308 to it; endpoints and files are left
/// as they are asked for, but a path ending in `/` that is no route's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrailingSlash {
    /// `/about/`.
    Always,
    /// `/about`, the default.
    Never,
    /// Either, served as it is asked for.
    Ignore,
}

pub(super) static SLASH: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(1);

/// `wisp::trailing_slash(Always)` in `init`: pages' addresses end in `/`,
/// and `/about` gets a 308 to `/about/`. `Never` is the default; `Ignore`
/// serves both. The build warns of a literal `href` of the other form.
pub fn trailing_slash(how: TrailingSlash) {
    SLASH.store(how as u8, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn slash() -> TrailingSlash {
    match SLASH.load(std::sync::atomic::Ordering::Relaxed) {
        0 => TrailingSlash::Always,
        2 => TrailingSlash::Ignore,
        _ => TrailingSlash::Never,
    }
}

/// A 308 to `cx`'s path with one leading slash, and a trailing one when
/// `trailing`, its query kept: `//evil.example/` would send the browser to
/// another site (and so would `/\evil.example/`).
pub(super) fn slash_redirect(cx: &Cx, reply: &mut Reply, trailing: bool) {
    let trimmed = cx.path().trim_matches(['/', '\\']);
    let end = if trailing && !trimmed.is_empty() {
        "/"
    } else {
        ""
    };
    let query = cx.query_string();
    let base = crate::protocol::BASE;
    let location = match query.is_empty() {
        true => format!("{base}/{trimmed}{end}"),
        false => format!("{base}/{trimmed}{end}?{query}"),
    };
    reply.set_plain(308, "");
    reply
        .headers
        .push((Cow::Borrowed("location"), Cow::Owned(location)));
    #[cfg(target_arch = "wasm32")]
    crate::edge::constant(cx);
}
