//! Security headers every page carries unless the app set its own:
//! `x-content-type-options: nosniff` and `referrer-policy`. `WISP_HSTS=on`
//! adds `strict-transport-security` to every answer, for a site served over
//! https (by the proxy in front); `WISP_SECURE_HEADERS=off` leaves all of
//! it out.
//!
//! Where a page is made is where it gets them ([`page`], `Response::html`),
//! so an answer that is not a page pays nothing. HSTS rides with the
//! request-id check at the start of a request, which a server with neither
//! on does not enter.

use crate::cx::Cx;
use std::borrow::Cow;

pub(crate) const NOSNIFF: (&str, &str) = ("x-content-type-options", "nosniff");
pub(crate) const REFERRER: (&str, &str) = ("referrer-policy", "strict-origin-when-cross-origin");
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

/// HSTS, unless the app set its own: once, as the request starts.
#[cold]
pub(crate) fn hsts(cx: &mut Cx) {
    if !cx.has_out(HSTS.0) {
        cx.put(HSTS.0, Cow::Borrowed(HSTS.1));
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
    fn hsts_gives_way_to_the_apps() {
        let mut a = cx();
        a.set_header("strict-transport-security", "max-age=1");
        hsts(&mut a);
        assert_eq!(a.out_headers().len(), 1);
        let mut b = cx();
        hsts(&mut b);
        b.set_header("strict-transport-security", "max-age=1");
        assert_eq!(b.out_headers().len(), 1);
        assert_eq!(b.out_headers()[0].1, "max-age=1");
    }
}
