//! `Idempotency-Key` on a POST: a client that sends one may send the same
//! request again (after a timeout, say) and get the first answer back rather
//! than a second order, note or payment. Answers are kept for a day, per key,
//! path and `authorization`, at most `MAX` of them; the same key with another
//! body is a 422, and one whose first request is still being answered a 409.
//! Only whole answers under 500 are kept: a stream, a page or a failure is
//! run again. Nothing is kept for requests without the header.

use crate::http::{Body, Reply};
use crate::{Cx, Error, Method};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Mutex;

const MAX: usize = 10_000;
const DAY: u64 = 24 * 60 * 60;

type Headers = Vec<(Cow<'static, str>, Cow<'static, str>)>;

struct Kept {
    /// The request's body, hashed: the same key must come with the same one.
    body: u64,
    at: u64,
    /// Status, headers and body; `None` while the first request is being
    /// answered.
    reply: Option<(u16, Headers, Vec<u8>)>,
}

static KEPT: Mutex<BTreeMap<u64, Kept>> = Mutex::new(BTreeMap::new());

fn fnv(parts: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for &b in *p {
            h = (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
        h = (h ^ 0xff).wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// What to do with a request that may carry a key.
pub(crate) enum Start {
    /// Answer it, then [`finish`] with this key.
    Fresh(u64),
    /// The first answer, again.
    Replay(Reply),
    Refused(Error),
}

/// `None` for a request without a key (or not a POST).
pub(crate) fn start(cx: &Cx) -> Option<Start> {
    if cx.method != Method::Post {
        return None;
    }
    let key = cx.header("idempotency-key")?;
    if key.is_empty() || key.len() > 255 {
        return Some(Start::Refused(Error::new(
            400,
            "Idempotency-Key must be 1 to 255 characters",
        )));
    }
    let auth = cx.header("authorization").unwrap_or("");
    let id = fnv(&[
        key.as_bytes(),
        cx.path().as_bytes(),
        cx.query_string().as_bytes(),
        auth.as_bytes(),
    ]);
    let body = fnv(&[cx.body()]);
    let now = crate::unix_now();
    let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    // A first request unanswered after a minute was dropped (its client
    // left, the task was cancelled): the key is free again.
    let live = |k: &&Kept| match k.reply {
        Some(_) => now.saturating_sub(k.at) < DAY,
        None => now.saturating_sub(k.at) < 60,
    };
    if let Some(k) = kept.get(&id).filter(live) {
        if k.body != body {
            return Some(Start::Refused(
                Error::new(422, "This Idempotency-Key was used with another request")
                    .with_code("idempotency_key_reused"),
            ));
        }
        return Some(match &k.reply {
            None => Start::Refused(
                Error::new(409, "A request with this Idempotency-Key is being answered")
                    .with_code("idempotency_key_in_use"),
            ),
            Some((status, headers, bytes)) => {
                let mut headers = headers.clone();
                headers.push((Cow::Borrowed("idempotent-replayed"), Cow::Borrowed("true")));
                Start::Replay(Reply {
                    status: *status,
                    headers,
                    body: Body::Bytes(bytes.clone()),
                })
            }
        });
    }
    if kept.len() >= MAX {
        kept.retain(|_, k| live(&&*k));
    }
    if kept.len() >= MAX
        && let Some(oldest) = kept.iter().min_by_key(|(_, k)| k.at).map(|(&id, _)| id)
    {
        kept.remove(&oldest);
    }
    kept.insert(
        id,
        Kept {
            body,
            at: now,
            reply: None,
        },
    );
    Some(Start::Fresh(id))
}

/// Keeps the answer to the request [`start`] gave `id`, or forgets the key
/// when the answer is not one to repeat.
pub(crate) fn finish(id: u64, reply: &Reply) {
    let bytes = match &reply.body {
        Body::Bytes(b) => Some(b.clone()),
        Body::Static(b) => Some(b.to_vec()),
        _ => None,
    };
    let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    match bytes.filter(|_| reply.status < 500) {
        Some(b) => {
            if let Some(k) = kept.get_mut(&id) {
                k.reply = Some((reply.status, reply.headers.clone(), b));
            }
        }
        None => {
            kept.remove(&id);
        }
    }
}
