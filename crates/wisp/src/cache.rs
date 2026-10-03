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

/// Most answers kept: past it, the expired go, then the oldest key.
const MAX: usize = 1024;

static KEPT: Shared<BTreeMap<String, (u64, Arc<dyn Any + Send + Sync>)>> =
    Shared::new(BTreeMap::new());

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
    let v = make().await;
    let mut kept = KEPT.lock();
    if kept.len() >= MAX && !kept.contains_key(key) {
        kept.retain(|_, (until, _)| *until > now);
        if kept.len() >= MAX {
            kept.pop_first();
        }
    }
    kept.insert(key.to_owned(), (now + secs, Arc::new(v.clone())));
    v
}

/// Forgets what is kept for a page and those below it: the answers of
/// [`cache`] whose key starts with `prefix`, and the pages `const CACHE`
/// keeps (on every worker thread) whose path is `prefix` or below it
/// (`/posts` is `/posts` and `/posts/1`, not `/postscript`; `/` is all).
pub fn uncache(prefix: &str) {
    KEPT.lock().retain(|k, _| !k.starts_with(prefix));
    crate::bake::purge(prefix);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn kept_until_it_expires_or_is_dropped() {
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
}
