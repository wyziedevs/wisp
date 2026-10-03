//! `/_wisp/health`, answered by the server for a load balancer or an
//! orchestrator: 200 while it takes requests, 503 once it is stopping, so
//! traffic moves away before the connections drain. Only a path that
//! starts `/_wisp/` is compared, after the ones Wisp already answers.

use crate::http::{Body, Reply};
use std::borrow::Cow;

pub(crate) const PATH: &str = "/_wisp/health";

/// The answer to a request for [`PATH`].
pub(crate) fn answer(reply: &mut Reply) {
    let (status, text) = if crate::http::stopping() {
        (503, &b"stopping"[..])
    } else {
        (200, &b"ok"[..])
    };
    reply.status = status;
    reply.headers.clear();
    reply.headers.push((
        Cow::Borrowed("content-type"),
        Cow::Borrowed("text/plain; charset=utf-8"),
    ));
    reply
        .headers
        .push((Cow::Borrowed("cache-control"), Cow::Borrowed("no-store")));
    reply.body = Body::Static(text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_ok() {
        let mut r = Reply::default();
        answer(&mut r);
        assert_eq!((r.status, r.text()), (200, "ok"));
        assert_eq!(r.header("cache-control"), Some("no-store"));
    }
}
