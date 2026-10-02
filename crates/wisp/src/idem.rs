//! `Idempotency-Key` on a POST: a client that sends one may send the same
//! request again (after a timeout, say) and get the first answer back rather
//! than a second order, note or payment. Answers are kept for a day, per key,
//! path, `authorization` and `cookie` (so one visitor never gets another's),
//! at most `MAX` of them and `BYTES` of their bodies; the same key with
//! another body is a 422, and one whose first request is still being
//! answered a 409. The key is looked at once the `before` hooks let the
//! request through, so a replay passes the same checks as the first. Only
//! whole answers that a retry would get again are kept (2xx, 3xx, and 400,
//! 404, 410, 422): anything else, a stream or a page frees the key, and the
//! request runs again. Nothing is kept for requests without the header.

use crate::http::{Body, Reply};
use crate::rest::hash;
use crate::{Cx, Error, Method, Response, Shared};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

const MAX: usize = 10_000;
const BYTES: usize = 64 * 1024 * 1024;
const DAY: u64 = 24 * 60 * 60;

/// An answer kept: status, headers (`content-type` among them) and body.
type Answer = (u16, Vec<(Cow<'static, str>, String)>, Box<[u8]>);

struct Kept {
    /// The request's body, hashed: the same key must come with the same one.
    body: u64,
    at: u64,
    /// Its place in `Keys::order`.
    seq: u64,
    /// `None` while the first request is being answered. Shared, so a
    /// replay copies it outside the lock.
    reply: Option<Arc<Answer>>,
}

struct Keys {
    map: BTreeMap<u64, Kept>,
    /// Each key by when it came (`seq`), oldest first: the next to go.
    order: BTreeMap<u64, u64>,
    seq: u64,
    /// The bodies of the answers kept.
    bytes: usize,
}

impl Keys {
    fn remove(&mut self, id: u64) {
        if let Some(k) = self.map.remove(&id) {
            self.order.remove(&k.seq);
            self.bytes -= k.reply.map_or(0, |r| r.2.len());
        }
    }

    /// The oldest go first: those a day old, then any while there are too
    /// many, or their answers take too much.
    fn trim(&mut self, now: u64) {
        while let Some((_, &first)) = self.order.first_key_value() {
            let old = self
                .map
                .get(&first)
                .is_none_or(|k| now.saturating_sub(k.at) >= DAY);
            if !old && self.map.len() < MAX && self.bytes <= BYTES {
                break;
            }
            self.remove(first);
        }
    }

    /// The key `seq` names, unless it went meanwhile (to make room):
    /// another request may have it now.
    fn id(&self, seq: u64) -> Option<u64> {
        self.order.get(&seq).copied()
    }
}

static KEPT: Shared<Keys> = Shared::new(Keys {
    map: BTreeMap::new(),
    order: BTreeMap::new(),
    seq: 0,
    bytes: 0,
});

/// What to do with a request that may carry a key.
pub(crate) enum Start {
    /// No key (or not a POST): answer it as usual.
    Skip,
    /// Answer it, then [`Key::finish`].
    Fresh(Key),
    /// What to send instead: the first answer again, or a refusal.
    Answered(Response),
}

/// A key whose first request is being answered. Dropped unfinished (the
/// request failed past its handler, or its connection went), it is freed.
pub(crate) struct Key(u64);

impl Drop for Key {
    fn drop(&mut self) {
        let mut keys = KEPT.lock();
        if let Some(id) = keys.id(self.0) {
            keys.remove(id);
        }
    }
}

impl Key {
    /// Keeps `reply`, with the headers the handler `set`, for the requests
    /// that come again with this key, or frees the key when the answer is
    /// not one to repeat.
    pub(crate) fn finish(self, reply: &Reply, set: &[(Cow<'static, str>, Cow<'static, str>)]) {
        let body = match &reply.body {
            Body::Bytes(b) => Some(&b[..]),
            Body::Static(b) => Some(*b),
            _ => None,
        };
        let Some(body) = body.filter(|_| matches!(reply.status, 200..=399 | 400 | 404 | 410 | 422))
        else {
            return; // dropped: freed
        };
        let headers = (reply.headers.iter().chain(set))
            .map(|(n, v)| (n.clone(), v.to_string()))
            .collect();
        let answer = Arc::new((reply.status, headers, body.into()));
        let mut keys = KEPT.lock();
        if let Some(id) = keys.id(self.0)
            && let Some(k) = keys.map.get_mut(&id)
        {
            k.reply = Some(answer);
            keys.bytes += body.len();
            keys.trim(crate::unix_now());
        }
        drop(keys);
        std::mem::forget(self); // kept, not freed
    }
}

/// A refusal, as JSON.
fn refused(e: Error) -> Start {
    let body = e.json(e.message(), false);
    Start::Answered(Response::json(body).with_status(e.status))
}

pub(crate) fn start(cx: &Cx) -> Start {
    if cx.method != Method::Post {
        return Start::Skip;
    }
    let Some(key) = cx.header("idempotency-key") else {
        return Start::Skip;
    };
    if key.is_empty() || key.len() > 255 {
        return refused(Error::new(
            400,
            "Idempotency-Key must be 1 to 255 characters",
        ));
    }
    // Seeded per process: no client can pick keys that collide.
    static SEED: OnceLock<[u8; 16]> = OnceLock::new();
    let id = hash(&[
        SEED.get_or_init(crate::sign::random),
        key.as_bytes(),
        cx.path().as_bytes(),
        cx.query_string().as_bytes(),
        cx.header("authorization").unwrap_or("").as_bytes(),
        cx.header("cookie").unwrap_or("").as_bytes(),
    ]);
    let body = hash(&[cx.body()]);
    let now = crate::unix_now();
    let mut kept = KEPT.lock();
    let live = |k: &&Kept| k.reply.is_none() || now.saturating_sub(k.at) < DAY;
    if let Some(k) = kept.map.get(&id).filter(live) {
        if k.body != body {
            return refused(
                Error::new(422, "This Idempotency-Key was used with another request")
                    .with_code("idempotency_key_reused"),
            );
        }
        let Some(reply) = k.reply.clone() else {
            return refused(
                Error::new(409, "A request with this Idempotency-Key is being answered")
                    .with_code("idempotency_key_in_use"),
            );
        };
        drop(kept);
        let (status, headers, bytes) = &*reply;
        let mut r = Response::new("", bytes.to_vec()).with_status(*status);
        r.headers.clone_from(headers);
        return Start::Answered(r.with_header("idempotent-replayed", "true"));
    }
    kept.remove(id);
    kept.trim(now);
    kept.seq += 1;
    let seq = kept.seq;
    kept.order.insert(seq, id);
    kept.map.insert(
        id,
        Kept {
            body,
            at: now,
            seq,
            reply: None,
        },
    );
    Start::Fresh(Key(seq))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fuzz::Fuzz;

    fn post(key: &str, body: &str) -> Cx {
        let headers = [("idempotency-key", key.as_bytes())];
        let peer = std::net::SocketAddr::from(([127, 0, 0, 1], 1));
        Cx::from_request::<Fuzz>("POST", "/p/a", headers, body.as_bytes(), peer).unwrap()
    }

    fn status(s: Start) -> u16 {
        match s {
            Start::Skip => 0,
            Start::Fresh(_) => 1,
            Start::Answered(r) => r.status,
        }
    }

    fn answered(status: u16) -> Reply {
        let mut r = Reply::default();
        (r.status, r.body) = (status, Body::Bytes(b"{}".to_vec()));
        r
    }

    #[test]
    fn a_kept_answer_comes_again() {
        let cx = post("kept", "a");
        let Start::Fresh(key) = start(&cx) else {
            panic!()
        };
        // Still being answered: the same request waits, another is refused.
        assert_eq!(status(start(&cx)), 409);
        assert_eq!(status(start(&post("kept", "b"))), 422);
        let set = [(Cow::Borrowed("set-cookie"), Cow::Borrowed("a=1"))];
        key.finish(&answered(201), &set);
        let Start::Answered(again) = start(&cx) else {
            panic!()
        };
        assert_eq!((again.status, &again.body[..]), (201, &b"{}"[..]));
        let has = |n: &str, v: &str| again.headers.iter().any(|(k, x)| k == n && x == v);
        assert!(has("set-cookie", "a=1") && has("idempotent-replayed", "true"));
    }

    #[test]
    fn a_failure_or_a_dropped_request_frees_its_key() {
        for status in [500, 503, 409, 429, 401, 403, 408] {
            let cx = post("failed", "a");
            let Start::Fresh(key) = start(&cx) else {
                panic!()
            };
            key.finish(&answered(status), &[]);
            assert!(matches!(start(&cx), Start::Fresh(_)), "{status}");
        }
        let cx = post("dropped", "a");
        drop(start(&cx));
        assert!(matches!(start(&cx), Start::Fresh(_)));
        // A page is not a whole answer: it runs again.
        let cx = post("page", "a");
        let Start::Fresh(key) = start(&cx) else {
            panic!()
        };
        let mut page = answered(200);
        page.body = Body::Page;
        key.finish(&page, &[]);
        assert!(matches!(start(&cx), Start::Fresh(_)));
    }
}
