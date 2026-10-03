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
