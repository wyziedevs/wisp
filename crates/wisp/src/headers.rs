//! Security headers every page carries unless the app set its own:
//! `x-content-type-options: nosniff` and `referrer-policy`. `WISP_HSTS=on`
//! adds `strict-transport-security` to every answer, for a site served over
//! https (by the proxy in front); `WISP_SECURE_HEADERS=off` leaves all of
//! it out. The headers are fixed static strings: no work per request but
//! the push.

use crate::http::Reply;
use std::borrow::Cow;
use std::sync::OnceLock;

const NOSNIFF: (&str, &str) = ("x-content-type-options", "nosniff");
const REFERRER: (&str, &str) = ("referrer-policy", "strict-origin-when-cross-origin");
const HSTS: (&str, &str) = (
    "strict-transport-security",
    "max-age=31536000; includeSubDomains",
);

#[derive(Clone, Copy)]
struct Mode {
    pages: bool,
    hsts: bool,
}

fn mode() -> Mode {
    static MODE: OnceLock<Mode> = OnceLock::new();
    *MODE.get_or_init(|| Mode {
        pages: crate::switch("WISP_SECURE_HEADERS", true),
        hsts: crate::switch("WISP_HSTS", false),
    })
}

/// The headers `reply` is missing: the page ones for HTML, which is what
/// pages and error pages are, HSTS for any.
pub(crate) fn add(reply: &mut Reply) {
    let m = mode();
    if reply.status < 200 || !(m.pages || m.hsts) {
        return;
    }
    let html = reply
        .headers
        .first()
        .is_some_and(|(n, v)| n == "content-type" && v.starts_with("text/html"));
    let wanted = [
        (m.pages && html, NOSNIFF),
        (m.pages && html, REFERRER),
        (m.hsts, HSTS),
    ];
    for (on, (name, value)) in wanted {
        if on && reply.header(name).is_none() {
            reply
                .headers
                .push((Cow::Borrowed(name), Cow::Borrowed(value)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_get_them_unless_set() {
        let page = || {
            let mut r = Reply::default();
            r.headers.push((
                Cow::Borrowed("content-type"),
                Cow::Borrowed("text/html; charset=utf-8"),
            ));
            r
        };
        let mut r = page();
        add(&mut r);
        assert_eq!(r.header("x-content-type-options"), Some("nosniff"));
        assert_eq!(
            r.header("referrer-policy"),
            Some("strict-origin-when-cross-origin")
        );
        assert_eq!(r.header("strict-transport-security"), None);
        let mut r = page();
        r.headers.push((
            Cow::Borrowed("referrer-policy"),
            Cow::Borrowed("no-referrer"),
        ));
        add(&mut r);
        assert_eq!(r.header("referrer-policy"), Some("no-referrer"));
        assert_eq!(r.headers.len(), 3);
    }

    #[test]
    fn json_and_switching_responses_are_left_alone() {
        let mut r = Reply::default();
        r.headers.push((
            Cow::Borrowed("content-type"),
            Cow::Borrowed("application/json"),
        ));
        add(&mut r);
        assert_eq!(r.headers.len(), 1);
        let mut r = Reply {
            status: 101,
            ..Reply::default()
        };
        add(&mut r);
        assert!(r.headers.is_empty());
    }
}
