//! Security headers every page carries unless the app set its own:
//! `x-content-type-options: nosniff` and `referrer-policy`. `WISP_HSTS=on`
//! adds `strict-transport-security` to every answer, for a site served over
//! https (by the proxy in front); `WISP_SECURE_HEADERS=off` leaves all of
//! it out.
//!
//! Where a page is made is where it gets them ([`page`], [`missing`]), so
//! an answer that is not a page pays one check of its content type. HSTS is
//! added to the reply last, as it is tagged ([`hsts`]), so every answer has
//! it, Wisp's own files and redirects too.

use crate::cx::Cx;
use crate::http::Reply;
use std::borrow::Cow;
use std::sync::atomic::AtomicBool;

pub(crate) const NOSNIFF: (&str, &str) = ("x-content-type-options", "nosniff");
pub(crate) const REFERRER: (&str, &str) = ("referrer-policy", "strict-origin-when-cross-origin");
/// `WISP_HSTS`, kept where a reply reads it in one load: every
/// answer is tagged, and a settings read is more than a load.
pub(crate) static HSTS_ON: AtomicBool = AtomicBool::new(false);
const HSTS: (&str, &str) = (
    "strict-transport-security",
    "max-age=31536000; includeSubDomains",
);

/// The page headers `cx` is missing, for a page or error page being made.
pub(crate) fn page(cx: &mut Cx) {
    if crate::settings().secure_headers {
        for (name, value) in [NOSNIFF, REFERRER] {
            if !cx.has_out(name) {
                cx.put(name, Cow::Borrowed(value));
            }
        }
    }
}

/// The page headers neither `cx` nor `own`, a response's, has set.
pub(crate) fn missing<'a>(
    cx: &'a Cx,
    own: &'a [(Cow<'static, str>, String)],
) -> impl Iterator<Item = (&'static str, &'static str)> + 'a {
    let on = crate::settings().secure_headers;
    [NOSNIFF, REFERRER].into_iter().filter(move |&(name, _)| {
        on && !cx.has_out(name) && !own.iter().any(|(n, _)| n.eq_ignore_ascii_case(name))
    })
}

/// HSTS, unless the reply has the app's own.
#[cold]
pub(crate) fn hsts(reply: &mut Reply) {
    if !reply
        .headers
        .iter()
        .any(|(n, _)| n.eq_ignore_ascii_case(HSTS.0))
    {
        reply
            .headers
            .push((Cow::Borrowed(HSTS.0), Cow::Borrowed(HSTS.1)));
    }
}

/// A response header, as `(name, value)`.
pub(crate) type Pair = (Cow<'static, str>, Cow<'static, str>);

/// A header the response has one of, which a second set replaces:
/// `content-type`, `cache-control`, `location`, `etag`, in any case.
/// (`content-length` and `transfer-encoding` the server writes itself and
/// leaves an app's out.)
pub(crate) fn single(name: &str) -> bool {
    ["content-type", "cache-control", "location", "etag"]
        .iter()
        .any(|s| name.eq_ignore_ascii_case(s))
}

/// Pushes `(name, value)` onto `list`, in place of one of the same name
/// when it is [`single`]: a reply's own list, with no savepoint.
pub(crate) fn set_in(list: &mut Vec<Pair>, name: &'static str, value: String) {
    if single(name) {
        list.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
    }
    list.push((Cow::Borrowed(name), Cow::Owned(value)));
}

/// The headers a request's handlers set, in order, with a savepoint
/// ([`Headers::mark`]) after the `before` hook's: those stay on an error
/// page ([`Headers::rollback`] truncates to it), a handler's go with the
/// page it did not finish. Reads as the slice of every one set.
#[derive(Default)]
pub(crate) struct Headers {
    list: Vec<Pair>,
    /// How many of `list` were set before the mark. A `u32` packs it with
    /// the flag beside it.
    mark: u32,
    /// [`Headers::set`] set a [`single`] header, which takes the place of a
    /// hook's or the response's own when it is sent.
    replaced: bool,
}

impl std::ops::Deref for Headers {
    type Target = [Pair];
    #[inline]
    fn deref(&self) -> &[Pair] {
        &self.list
    }
}

impl Headers {
    /// Empty, for the next request; the buffer is kept.
    #[inline]
    pub(crate) fn clear(&mut self) {
        self.list.clear();
        self.mark = 0;
        self.replaced = false;
    }

    /// Sets a header: a [`single`] one replaces the one set since the mark.
    /// One set before it stays, shadowed, so a rollback brings it back.
    pub(crate) fn set(&mut self, name: Cow<'static, str>, value: Cow<'static, str>) {
        if single(&name) {
            let (mark, mut at) = (self.mark as usize, 0);
            self.list.retain(|(n, _)| {
                at += 1;
                at <= mark || !n.eq_ignore_ascii_case(&name)
            });
            self.replaced = true;
        }
        self.list.push((name, value));
    }

    /// Adds a header, whatever was set before.
    #[inline]
    pub(crate) fn append(&mut self, name: Cow<'static, str>, value: Cow<'static, str>) {
        self.list.push((name, value));
    }

    /// Whether a header of `name` is set.
    pub(crate) fn has(&self, name: &str) -> bool {
        (self.list.iter()).any(|(n, _)| n.eq_ignore_ascii_case(name))
    }

    /// The savepoint: the headers set so far stay through a rollback.
    #[inline]
    pub(crate) fn mark(&mut self) {
        self.mark = self.list.len() as u32;
    }

    /// Drops the headers set since the mark.
    #[inline]
    pub(crate) fn rollback(&mut self) {
        self.list.truncate(self.mark as usize);
    }

    /// The headers set since the mark.
    #[inline]
    pub(crate) fn since_mark(&self) -> &[Pair] {
        &self.list[self.mark as usize..]
    }

    /// Takes [`Headers::since_mark`] out; one set before the mark that one
    /// since replaced is dropped.
    #[inline]
    pub(crate) fn take_since_mark(&mut self) -> std::vec::Drain<'_, Pair> {
        if self.replaced {
            self.shadow();
        }
        self.list.drain(self.mark as usize..)
    }

    /// Moves every header onto the end of `to`, in place of a [`single`]
    /// one there or set before the mark that was set again.
    #[inline]
    pub(crate) fn send(&mut self, to: &mut Vec<Pair>) {
        if self.replaced {
            self.shadow();
            let set = |n: &str| (self.list.iter()).any(|(o, _)| o.eq_ignore_ascii_case(n));
            to.retain(|(n, _)| !(single(n) && set(n)));
        }
        to.append(&mut self.list);
    }

    /// Drops the [`single`] headers set before the mark that were set
    /// again since: the page worked, so the later ones stand.
    #[cold]
    fn shadow(&mut self) {
        let mark = self.mark as usize;
        let (before, since) = self.list.split_at(mark);
        let set = |n: &str| (since.iter()).any(|(o, _)| o.eq_ignore_ascii_case(n));
        let gone: Vec<bool> = before.iter().map(|(n, _)| single(n) && set(n)).collect();
        let mut at = 0;
        self.list.retain(|_| {
            at += 1;
            !gone.get(at - 1).copied().unwrap_or(false)
        });
        self.mark -= gone.iter().filter(|g| **g).count() as u32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cx() -> Cx {
        Cx::new("127.0.0.1:1".parse().unwrap())
    }

    #[test]
    fn pages_get_them_unless_set() {
        let mut a = cx();
        page(&mut a);
        assert_eq!(a.out_headers().len(), 2);
        let mut b = cx();
        b.set_header("referrer-policy", "no-referrer");
        page(&mut b);
        let named = |n: &str| b.out_headers().iter().filter(|(k, _)| k == n).count();
        assert_eq!(
            (named("referrer-policy"), named("x-content-type-options")),
            (1, 1)
        );
    }

    #[test]
    fn missing_ones_only() {
        let mut a = cx();
        a.set_header("referrer-policy", "no-referrer");
        let own = vec![(
            Cow::Borrowed("X-Content-Type-Options"),
            "nosniff".to_string(),
        )];
        assert_eq!(missing(&a, &own).count(), 0);
        assert_eq!(missing(&a, &[]).count(), 1);
        assert_eq!(missing(&cx(), &[]).count(), 2);
    }

    #[test]
    fn hsts_gives_way_to_the_apps() {
        let mut reply = Reply::default();
        reply.headers.push((
            Cow::Borrowed("Strict-Transport-Security"),
            Cow::Borrowed("max-age=1"),
        ));
        hsts(&mut reply);
        assert_eq!(reply.headers.len(), 1);
        let mut reply = Reply::default();
        hsts(&mut reply);
        assert_eq!(reply.headers.len(), 1);
    }
}
