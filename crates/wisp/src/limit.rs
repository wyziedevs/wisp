//! Rate limits: so many requests per key (a client's IP, an API key) in a
//! window, kept in memory as token buckets.
//!
//! In process only: several servers of one app each count their own, and
//! the edge build, where every request may be its own instance, has none
//! (edge hosts limit rates themselves).

use crate::{Error, Result, Shared};
use std::collections::HashMap;
use std::hash::{BuildHasher, BuildHasherDefault, Hash, Hasher, RandomState};
use std::sync::OnceLock;
use std::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
#[cfg(target_arch = "wasm32")]
use wasm_clock::Instant;

/// `std`'s `Instant` has no clock on the edge: the host's, as a time since 1970.
#[cfg(target_arch = "wasm32")]
mod wasm_clock {
    use std::ops::Add;
    use std::time::Duration;

    #[derive(Clone, Copy, PartialEq, PartialOrd)]
    pub struct Instant(Duration);

    impl Instant {
        pub fn now() -> Instant {
            Instant(crate::edge::clock())
        }

        pub fn saturating_duration_since(self, earlier: Instant) -> Duration {
            self.0.saturating_sub(earlier.0)
        }
    }

    impl Add<Duration> for Instant {
        type Output = Instant;

        fn add(self, d: Duration) -> Instant {
            Instant(self.0 + d)
        }
    }
}

/// Past this many keys, buckets that have filled up again are dropped, so
/// clients that came once do not stay in memory. If that is not enough
/// (more keys than this in one window: many addresses, or made-up keys),
/// others are forgotten too, so memory stays bounded, and each forgotten
/// key starts again with a full bucket.
const KEYS: usize = 10_000;

/// Keys are spread over this many maps, each with a lock of its own, so
/// requests with different keys seldom wait on each other.
const SHARDS: usize = 16;

/// A key's hash, keyed per process so clients cannot pick keys that pile
/// into one place of a map.
fn hash(key: impl Hash) -> u64 {
    static SEED: OnceLock<RandomState> = OnceLock::new();
    SEED.get_or_init(RandomState::new).hash_one(key)
}

/// The maps' keys are hashes already: they are used as they are.
#[derive(Default)]
struct AsIs(u64);

impl Hasher for AsIs {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = self.0.rotate_left(8) ^ b as u64;
        }
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = n;
    }
}

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
    shards: [Shared<Buckets>; SHARDS],
}

/// Per key's hash: tokens left, and when they were counted.
struct Buckets {
    map: HashMap<u64, (f64, Instant), BuildHasherDefault<AsIs>>,
    /// The earliest the next sweep may run: at most one a second, so a map
    /// that stays full is not walked on every request.
    sweep: Option<Instant>,
}

impl RateLimit {
    pub const fn new(requests: u32, window: Duration) -> RateLimit {
        assert!(requests > 0, "a rate limit allows at least one request");
        RateLimit {
            requests,
            window,
            shards: [const {
                Shared::new(Buckets {
                    map: HashMap::with_hasher(BuildHasherDefault::new()),
                    sweep: None,
                })
            }; SHARDS],
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
        let h = hash(key);
        let full = self.requests as f64;
        let rate = full / self.window.as_secs_f64(); // tokens a second
        // The map's places come from the hash's low bits: shards by others.
        let mut guard = self.shards[(h >> 32) as usize % SHARDS].lock();
        let Buckets { map, sweep } = &mut *guard;
        let keys = KEYS / SHARDS;
        if map.len() >= keys && sweep.is_none_or(|s| now >= s) {
            map.retain(|_, (_, at)| now.saturating_duration_since(*at) < self.window);
            *sweep = Some(now + Duration::from_secs(1));
        }
        // Keys are hashes, so any are as good as random to forget.
        if map.len() >= 2 * keys {
            let mut excess = map.len() + 1 - 2 * keys;
            map.retain(|_, _| {
                let keep = excess == 0;
                excess = excess.saturating_sub(1);
                keep
            });
        }
        let (tokens, at) = map.entry(h).or_insert((full, now));
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
        assert_eq!(e.header.as_deref(), Some(&("retry-after", "30".into())));
        assert!(limit.take("b", t).is_ok(), "each key has its own");
        assert!(limit.take("a", t + Duration::from_secs(29)).is_err());
        assert!(
            limit.take("a", t + Duration::from_secs(31)).is_ok(),
            "one back every 30 s"
        );
        assert!(limit.take("a", t + Duration::from_secs(32)).is_err());
    }

    #[test]
    fn many_keys_stay_bounded() {
        let limit = RateLimit::per_hour(1);
        let t = Instant::now();
        let keys = || {
            limit
                .shards
                .iter()
                .map(|s| s.lock().map.len())
                .sum::<usize>()
        };
        for k in 0..5 * KEYS {
            assert!(limit.take(k, t).is_ok());
        }
        assert!(keys() <= 2 * KEYS);
        // Each shard sweeps when a key of its own comes.
        let later = t + Duration::from_secs(3601);
        for k in 0..20 * SHARDS {
            assert!(limit.take(("new", k), later).is_ok());
        }
        assert!(keys() <= 20 * SHARDS, "all filled up again");
    }
}
