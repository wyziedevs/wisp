//! `Idempotency-Key` on a POST: a client that sends one may send the same
//! request again (after a timeout, say) and get the first answer back rather
//! than a second order, note or payment. Answers are kept for a day, per key,
//! path, `authorization` and `cookie` (so one visitor never gets another's),
//! at most `MAX` of them and `BYTES` of their bodies; the same key with
//! another body is a 422, and one whose first request is still being
//! answered a 409. Only whole answers under 500 are kept: a stream, a page
//! or a failure is run again. Nothing is kept for requests without the
//! header.

use crate::http::{Body, Reply};
use crate::rest::hash;
use crate::{Cx, Error, Method, Shared};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

const MAX: usize = 10_000;
const BYTES: usize = 64 * 1024 * 1024;
const DAY: u64 = 24 * 60 * 60;

/// An answer kept: status, headers and body.
type Answer = (u16, Vec<(Cow<'static, str>, Cow<'static, str>)>, Box<[u8]>);

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
    /// Answer it, then [`finish`] with this.
    Fresh(u64),
    /// The first answer, again.
    Replay(Reply),
    Refused(Error),
}

pub(crate) fn start(cx: &Cx) -> Start {
    if cx.method != Method::Post {
        return Start::Skip;
    }
    let Some(key) = cx.header("idempotency-key") else {
        return Start::Skip;
    };
    if key.is_empty() || key.len() > 255 {
        return Start::Refused(Error::new(
            400,
            "Idempotency-Key must be 1 to 255 characters",
        ));
    }
    let id = hash(&[
        key.as_bytes(),
        cx.path().as_bytes(),
        cx.query_string().as_bytes(),
        cx.header("authorization").unwrap_or("").as_bytes(),
        cx.header("cookie").unwrap_or("").as_bytes(),
    ]);
    let body = hash(&[cx.body()]);
    let now = crate::unix_now();
    // A first request unanswered after a minute was dropped (its client
    // left, the task was cancelled): the key is free again.
    let live = |k: &Kept| match k.reply {
        Some(_) => now.saturating_sub(k.at) < DAY,
        None => now.saturating_sub(k.at) < 60,
    };
    let mut kept = KEPT.lock();
    if let Some(k) = kept.map.get(&id).filter(|k| live(k)) {
        if k.body != body {
            return Start::Refused(
                Error::new(422, "This Idempotency-Key was used with another request")
                    .with_code("idempotency_key_reused"),
            );
        }
        let Some(reply) = k.reply.clone() else {
            return Start::Refused(
                Error::new(409, "A request with this Idempotency-Key is being answered")
                    .with_code("idempotency_key_in_use"),
            );
        };
        drop(kept);
        let (status, headers, bytes) = &*reply;
        let mut headers = headers.clone();
        headers.push((Cow::Borrowed("idempotent-replayed"), Cow::Borrowed("true")));
        return Start::Replay(Reply {
            status: *status,
            headers,
            body: Body::Bytes(bytes.to_vec()),
        });
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
    Start::Fresh(seq)
}

/// Keeps the answer to the request [`start`] gave `seq`, or forgets its key
/// when the answer is not one to repeat. A key that went meanwhile (to make
/// room) stays gone: another request may have it now.
pub(crate) fn finish(seq: u64, reply: &Reply) {
    let bytes: Option<Box<[u8]>> = match &reply.body {
        Body::Bytes(b) => Some(b.as_slice().into()),
        Body::Static(b) => Some((*b).into()),
        _ => None,
    };
    let kept = bytes
        .filter(|_| reply.status < 500)
        .map(|b| Arc::new((reply.status, reply.headers.clone(), b)));
    let mut guard = KEPT.lock();
    let keys = &mut *guard;
    let Some(&id) = keys.order.get(&seq) else {
        return;
    };
    match kept {
        Some(r) => {
            let len = r.2.len();
            if let Some(k) = keys.map.get_mut(&id) {
                k.reply = Some(r);
                keys.bytes += len;
            }
            keys.trim(crate::unix_now());
        }
        None => keys.remove(id),
    }
}
