//! Rate limits: so many requests per key (a client's IP, an API key) in a
//! window, kept in memory as token buckets.
//!
//! In process only: several servers of one app each count their own, and
//! the edge build, where every request may be its own instance, has none
//! (edge hosts limit rates themselves).

use crate::{Error, Result};
use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Past this many keys, buckets that have filled up again are dropped, so
/// clients that came once do not stay in memory.
const KEYS: usize = 10_000;

/// `requests` per `window` for each key, as a static:
///
/// ```ignore
/// static LOGINS: RateLimit = RateLimit::per_minute(10);
///
/// fn post(cx: &mut Cx, name: String) -> Result<()> {
///     LOGINS.check(cx.client_ip())?; // a 429 once the ten are used
///     ...
/// }
/// ```
///
/// A key may use all of them at once, then gets them back steadily over
/// the window (a token bucket).
pub struct RateLimit {
    requests: u32,
    window: Duration,
    /// Per key's hash: tokens left, and when they were counted.
    buckets: Mutex<BTreeMap<u64, (f64, Instant)>>,
}

impl RateLimit {
    pub const fn new(requests: u32, window: Duration) -> RateLimit {
        assert!(requests > 0, "a rate limit allows at least one request");
        RateLimit {
            requests,
            window,
            buckets: Mutex::new(BTreeMap::new()),
        }
    }

    pub const fn per_second(requests: u32) -> RateLimit {
        RateLimit::new(requests, Duration::from_secs(1))
    }

    pub const fn per_minute(requests: u32) -> RateLimit {
        RateLimit::new(requests, Duration::from_secs(60))
    }

    pub const fn per_hour(requests: u32) -> RateLimit {
        RateLimit::new(requests, Duration::from_secs(3600))
    }

    /// Counts a request for `key`: `Ok` while it has some left, then a 429
    /// with `retry-after` (the seconds until it has one again).
    pub fn check(&self, key: impl Hash) -> Result<()> {
        self.take(key, Instant::now())
    }

    fn take(&self, key: impl Hash, now: Instant) -> Result<()> {
        let mut h = DefaultHasher::new();
        key.hash(&mut h);
        let full = self.requests as f64;
        let rate = full / self.window.as_secs_f64(); // tokens a second
        let mut buckets = self.buckets.lock().unwrap_or_else(|e| e.into_inner());
        if buckets.len() >= KEYS {
            buckets.retain(|_, (_, at)| now.saturating_duration_since(*at) < self.window);
        }
        let (tokens, at) = buckets.entry(h.finish()).or_insert((full, now));
        *tokens = (*tokens + now.saturating_duration_since(*at).as_secs_f64() * rate).min(full);
        *at = now;
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            return Ok(());
        }
        let wait = ((1.0 - *tokens) / rate).ceil().max(1.0) as u64;
        Err(Error::new(429, "Too Many Requests").with_header("retry-after", wait.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets() {
        let limit = RateLimit::per_minute(2);
        let t = Instant::now();
        assert!(limit.take("a", t).is_ok() && limit.take("a", t).is_ok());
        let e = limit.take("a", t).unwrap_err();
        assert_eq!(e.status(), 429);
        assert_eq!(e.header, Some(("retry-after", "30".into())));
        assert!(limit.take("b", t).is_ok(), "each key has its own");
        assert!(limit.take("a", t + Duration::from_secs(29)).is_err());
        assert!(
            limit.take("a", t + Duration::from_secs(31)).is_ok(),
            "one back every 30 s"
        );
        assert!(limit.take("a", t + Duration::from_secs(32)).is_err());
    }
}
