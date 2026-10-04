//! A cache for what is slow to make: `wisp::cache("top", 60, || async {
//! ... })` keeps the answer for 60 seconds, and `wisp::uncache("/posts")`
//! forgets it, and the pages `const CACHE` keeps, when the data changes.
//!
//! Per process, shared by all its threads: with several servers, each has
//! its own, and `uncache` reaches only the one it runs on (`wisp::relay`
//! can tell the others). It does not make one call of many at once: a
//! flood after the answer expires makes each of them.

use crate::Shared;
use std::any::Any;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// Most answers kept: past it, the expired go, then the oldest key.
const MAX: usize = 1024;

/// An answer and the unix second it is kept until.
type Kept = (u64, Arc<dyn Any + Send + Sync>);

/// How many `uncache`s there were, and the last few's prefixes by number:
/// an answer made across one for its key is stale, and not kept.
static FORGOTTEN: AtomicU64 = AtomicU64::new(0);
static FORGOT: Shared<Vec<(u64, String)>> = Shared::new(Vec::new());

/// Most prefixes remembered: an answer made across more is not kept.
const FORGOT_KEPT: usize = 16;

/// Whether `key` was uncached since `FORGOTTEN` was `from`.
fn forgotten(key: &str, from: u64) -> bool {
    if FORGOTTEN.load(Relaxed) == from {
        return false;
    }
    let log = FORGOT.lock();
    log.first().is_none_or(|(n, _)| *n > from + 1)
        || log
            .iter()
            .any(|(n, p)| *n > from && key.starts_with(p.as_str()))
}

static KEPT: Shared<BTreeMap<String, Kept>> = Shared::new(BTreeMap::new());

/// The answer kept under `key` if it is under `secs` seconds old, else what
/// `make` gives, kept from now: `let top = wisp::cache("top", 60, || async {
/// top_posts().await }).await;`. A key holds one type: the same key with
/// another is made again.
pub async fn cache<T, F, Fut>(key: &str, secs: u64, make: F) -> T
where
    T: Clone + Send + Sync + 'static,
    F: FnOnce() -> Fut,
    Fut: Future<Output = T>,
{
    let now = crate::unix_now();
    let hit = KEPT
        .lock()
        .get(key)
        .filter(|(until, _)| *until > now)
        .and_then(|(_, v)| v.downcast_ref::<T>().cloned());
    if let Some(v) = hit {
        return v;
    }
    let from = FORGOTTEN.load(Relaxed);
    let v = make().await;
    let mut kept = KEPT.lock();
    if secs == 0 || forgotten(key, from) {
        return v;
    }
    if kept.len() >= MAX && !kept.contains_key(key) {
        kept.retain(|_, (until, _)| *until > now);
        // Still full: the one that expires first goes.
        let oldest = kept.iter().min_by_key(|(_, (until, _))| *until);
        if let Some(key) = oldest.map(|(k, _)| k.clone()).filter(|_| kept.len() >= MAX) {
            kept.remove(&key);
        }
    }
    kept.insert(
        key.to_owned(),
        (now.saturating_add(secs), Arc::new(v.clone())),
    );
    v
}

/// Forgets what is kept for a page and those below it: the answers of
/// [`cache`] whose key starts with `prefix`, and the pages `const CACHE`
/// keeps (on every worker thread) whose path is `prefix` or below it
/// (`/posts` is `/posts` and `/posts/1`, not `/postscript`; `/` is all).
pub fn uncache(prefix: &str) {
    let mut kept = KEPT.lock();
    let mut log = FORGOT.lock();
    log.push((FORGOTTEN.load(Relaxed) + 1, prefix.to_owned()));
    if log.len() > FORGOT_KEPT {
        log.remove(0);
    }
    FORGOTTEN.fetch_add(1, Relaxed);
    drop(log);
    kept.retain(|k, _| !k.starts_with(prefix));
    drop(kept);
    crate::bake::purge(prefix);
}

/// Drops every page `CACHE` keeps under `tag` (`const CACHE_TAGS` of its
/// route, or `cx.cache_tag` in its handler), on every worker thread, before
/// each next answers from what it keeps: `wisp::revalidate_tag("posts")`
/// after a post changes. Costs nothing to a lookup.
pub fn revalidate_tag(tag: &str) {
    crate::bake::purge_tag(tag);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// The tests share `KEPT` (a flood in one evicts another's keys, an
    /// `uncache` drops what is being made): one at a time.
    static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn kept_until_it_expires_or_is_dropped() {
        let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let made = AtomicU32::new(0);
        let get = |key: &str, secs| {
            rt.block_on(cache(key, secs, || async {
                made.fetch_add(1, Ordering::Relaxed) + 1
            }))
        };
        assert_eq!((get("cache-a", 60), get("cache-a", 60)), (1, 1));
        assert_eq!(get("cache-b", 60), 2, "another key");
        assert_eq!(get("cache-c", 0), 3);
        assert_eq!(get("cache-c", 0), 4, "no time at all keeps nothing");
        uncache("cache-a");
        assert_eq!(get("cache-a", 60), 5);
        assert_eq!(get("cache-b", 60), 2, "other prefixes stay");
        // One key, another type: made again.
        let s = rt.block_on(cache("cache-b", 60, || async { "text".to_string() }));
        assert_eq!(s, "text");
        for i in 0..MAX + 10 {
            get(&format!("cache-flood-{i}"), 60);
        }
        assert!(KEPT.lock().len() <= MAX);
    }

    #[test]
    fn an_answer_made_across_an_uncache_is_not_kept() {
        let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let v = rt.block_on(cache("stale-a", 60, || async {
            uncache("stale-a");
            1u8
        }));
        assert_eq!(v, 1, "the caller still gets it");
        assert!(!KEPT.lock().contains_key("stale-a"));
        let v = rt.block_on(cache("stale-b", 60, || async {
            uncache("other");
            2u8
        }));
        assert!(
            KEPT.lock().contains_key("stale-b") && v == 2,
            "another prefix"
        );
    }

    #[test]
    fn the_one_that_expires_first_goes_when_it_is_full() {
        let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let get = |key: &str, secs| rt.block_on(cache(key, secs, || async { 1u8 }));
        // `zzz` sorts last but expires first; the rest outlive it.
        get("evict-zzz", 30);
        for i in 0..MAX {
            get(&format!("evict-{i}"), 3000);
        }
        assert!(!KEPT.lock().contains_key("evict-zzz"));
        assert!(KEPT.lock().contains_key("evict-0"));
        // A time too long to add is kept as long as there is.
        assert_eq!(get("evict-huge", u64::MAX), 1);
    }
}
