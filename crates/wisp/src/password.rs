//! Passwords, kept as hashes: `let hash = wisp::password::hash(&password).await?;`
//! when one is chosen, `wisp::password::check(&password, hash).await?` when
//! it is typed again.
//!
//! The hash is PBKDF2-HMAC-SHA256 (RFC 8018) with a random 16-byte salt and
//! 600,000 rounds, what OWASP asks of it, written with its rounds in the
//! PHC string form: `$pbkdf2-sha256$i=600000$<salt>$<key>` (base64, no
//! padding). [`check`] reads the rounds from the hash, so hashes made with
//! fewer (or, later, more) still check; [`outdated`] says which to make
//! again, from the password, the next time it is typed.
//!
//! A hash takes about a fifth of a second of one core, on purpose: that is
//! what makes guessing slow. It runs on a thread kept for hashing, one for
//! every two cores, so the request's worker goes on answering its other
//! connections meanwhile, and a flood of sign-ins takes at most half the
//! machine. Past about 3 s of hashes queued, by their rounds, one more is
//! a 503 with `retry-after` rather than a longer wait: hashes planted with
//! many rounds cannot make the wait longer. That keeps the machine
//! answering, not sign-ins working, through a flood: a `RateLimit` on the
//! sign-in action, by name and by address, is the defence against one. The
//! edge build has no threads and hashes in place.

use crate::Result;
use crate::sign::{base64, pbkdf2, random, unbase64};

/// The rounds [`hash`] uses.
pub const ROUNDS: u32 = 600_000;

/// The most rounds [`check`] spends on a hash, about 3 s of a core: a
/// stored hash that names more (damaged, or planted) is `false` rather than
/// a worker thread held for hours.
const MAX_ROUNDS: u32 = 10_000_000;

const PREFIX: &str = "$pbkdf2-sha256$i=";

/// Hashes of [`ROUNDS`] queued per hashing thread, about 3 s of work,
/// before one more is a 503: counted in rounds, so a hash of more counts
/// for more.
#[cfg(not(target_arch = "wasm32"))]
const QUEUED: usize = 16;

/// `password`, hashed with a salt of its own, as text to keep.
pub async fn hash(password: &str) -> Result<String> {
    let salt = random::<16>();
    let password = password.as_bytes().to_vec();
    let key = off_worker(ROUNDS, move || pbkdf2(&password, &salt, ROUNDS)).await?;
    Ok(encode(ROUNDS, &salt, &key))
}

/// Whether `password` is the one `hash` (made by [`hash`]) was made from,
/// in time that does not depend on where they differ, nor on whether there
/// is a hash at all: `None` (no such user), or text [`hash`] does not make,
/// takes as long as a hash of now and is `false`. So a sign-in that says
/// one thing for both tells nobody which names exist:
///
/// ```ignore
/// let user = USERS.find(|u| u.name == name);
/// let hash = user.as_ref().map(|u| u.hash.as_str());
/// if !wisp::password::check(&password, hash).await? {
///     return invalid("password", "Wrong name or password");
/// }
/// ```
pub async fn check(password: &str, hash: Option<&str>) -> Result<bool> {
    let found = hash.and_then(parse);
    let real = found.is_some();
    let (rounds, salt, key) = found.unwrap_or((ROUNDS, b"wisp: nobody".to_vec(), [0; 32]));
    let password = password.as_bytes().to_vec();
    let made = off_worker(rounds, move || pbkdf2(&password, &salt, rounds)).await?;
    Ok(crate::secure_eq(made, key) & real)
}

/// Whether `hash` was made with fewer rounds than [`hash`] uses now, or is
/// not one of its hashes: make it again once the password checks.
///
/// ```ignore
/// if wisp::password::outdated(&user.hash) {
///     let hash = wisp::password::hash(&password).await?;
///     USERS.update(user.id, |u| u.hash = hash);
/// }
/// ```
pub fn outdated(hash: &str) -> bool {
    parse(hash).is_none_or(|(rounds, ..)| rounds < ROUNDS)
}

/// A hash's rounds, salt and key; `None` for any text [`hash`] does not
/// make, or one naming more than [`MAX_ROUNDS`].
fn parse(hash: &str) -> Option<(u32, Vec<u8>, [u8; 32])> {
    let mut parts = hash.strip_prefix(PREFIX)?.split('$');
    let (Some(rounds), Some(salt), Some(key), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    let rounds = rounds.parse::<u32>().ok()?;
    let (mut salt_bytes, mut key_bytes) = ([0u8; 64], [0u8; 32]);
    let salt_len = unbase64(salt, &mut salt_bytes)?;
    if !(1..=MAX_ROUNDS).contains(&rounds) || unbase64(key, &mut key_bytes) != Some(32) {
        return None;
    }
    Some((rounds, salt_bytes[..salt_len].to_vec(), key_bytes))
}

/// `work`'s answer, a hash of `rounds`, worked out on one of the threads
/// kept for hashing while the caller's worker serves its other connections.
/// The threads start with the first hash; where none will start, or in the
/// edge build, it runs here. A full queue (in rounds, or in hashes) is a
/// 503, tried again in a second, but an empty one always takes a hash;
/// work whose caller is gone (its request dropped) is skipped.
#[cfg(not(target_arch = "wasm32"))]
async fn off_worker<T: Send + 'static>(
    rounds: u32,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T> {
    use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    use std::sync::mpsc::{self, TrySendError};
    use std::sync::{Arc, Mutex, OnceLock};
    type Job = Box<dyn FnOnce() + Send>;
    /// The rounds of the hashes queued or running.
    static OWED: AtomicU64 = AtomicU64::new(0);
    /// A job's rounds, given back to [`OWED`] once it ran or was dropped.
    struct Owed(u64);
    impl Drop for Owed {
        fn drop(&mut self) {
            OWED.fetch_sub(self.0, Relaxed);
        }
    }
    static POOL: OnceLock<Option<(mpsc::SyncSender<Job>, u64)>> = OnceLock::new();
    let pool = POOL.get_or_init(|| {
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get().div_ceil(2));
        let (send, take) = mpsc::sync_channel::<Job>(threads * QUEUED);
        let take = Arc::new(Mutex::new(take));
        let mut started = 0;
        for i in 0..threads {
            let take = Arc::clone(&take);
            let thread = std::thread::Builder::new()
                .name(format!("wisp-hash-{i}"))
                .spawn(move || {
                    loop {
                        // The lock is let go before the job runs.
                        let job = take.lock().unwrap_or_else(|e| e.into_inner()).recv();
                        match job {
                            Ok(job) => job(),
                            Err(_) => return,
                        }
                    }
                });
            started += usize::from(thread.is_ok());
        }
        let budget = (started * QUEUED) as u64 * u64::from(ROUNDS);
        (started > 0).then_some((send, budget))
    });
    let busy = || {
        let busy = crate::Error::new(503, "Too many passwords to check at once");
        Err(busy.with_header("retry-after", "1"))
    };
    let owed = u64::from(rounds);
    let before = OWED.fetch_add(owed, Relaxed);
    let owed = Owed(owed);
    if pool
        .as_ref()
        .is_some_and(|(_, budget)| !admits(before, owed.0, *budget))
    {
        return busy();
    }
    let (done, answer) = tokio::sync::oneshot::channel();
    // A panic comes back to the caller, as if the work had run there; the
    // thread goes on.
    let job: Job = Box::new(move || {
        let _owed = owed;
        if !done.is_closed() {
            let _ = done.send(catch_unwind(AssertUnwindSafe(work)));
        }
    });
    let unsent = match pool {
        Some((pool, _)) => match pool.try_send(job) {
            Ok(()) => None,
            Err(TrySendError::Full(_)) => return busy(),
            Err(TrySendError::Disconnected(job)) => Some(job),
        },
        None => Some(job),
    };
    if let Some(job) = unsent {
        job();
    }
    match answer.await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(panic)) => resume_unwind(panic),
        Err(_) => panic!("a password hash was dropped before it ran"),
    }
}

/// Whether a hash of `rounds` joins `queued` rounds of hashes, within
/// `budget`: always into an empty queue, so a hash of more than the budget
/// still runs, alone.
#[cfg(not(target_arch = "wasm32"))]
fn admits(queued: u64, rounds: u64, budget: u64) -> bool {
    queued == 0 || queued + rounds <= budget
}

#[cfg(target_arch = "wasm32")]
async fn off_worker<T>(_: u32, work: impl FnOnce() -> T) -> Result<T> {
    Ok(work())
}

/// The PHC string of a hash.
fn encode(rounds: u32, salt: &[u8], key: &[u8; 32]) -> String {
    let mut out = format!("{PREFIX}{rounds}$");
    base64(&mut out, salt, false);
    out.truncate(out.trim_end_matches('=').len());
    out.push('$');
    base64(&mut out, key, false);
    out.truncate(out.trim_end_matches('=').len());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn block<F: Future>(f: F) -> F::Output {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(f)
    }

    fn verify(password: &str, hash: &str) -> bool {
        block(check(password, Some(hash))).unwrap()
    }

    #[test]
    fn hashes_check_and_carry_their_rounds() {
        let h = block(hash("correct horse")).unwrap();
        assert!(h.starts_with("$pbkdf2-sha256$i=600000$"), "{h}");
        assert_eq!(h.len(), PREFIX.len() + 6 + 1 + 22 + 1 + 43);
        assert!(verify("correct horse", &h));
        assert!(!verify("correct horsf", &h));
        assert_ne!(block(hash("correct horse")).unwrap(), h, "salted");

        // Fewer rounds, as an older hash might have: still checked, by its own count.
        let salt = b"0123456789abcdef";
        let old = encode(1000, salt, &pbkdf2(b"pw", salt, 1000));
        assert!(old.starts_with("$pbkdf2-sha256$i=1000$MDEyMzQ1Njc4OWFiY2RlZg$"));
        assert!(verify("pw", &old) && !verify("pW", &old));
        let key = old.rsplit('$').next().unwrap();
        for bad in [
            "",
            "pw",
            &old.replace("i=1000", "i=999"),
            &old.replace("i=1000", "i=0"),
            &old.replace("i=1000", "i=x"),
            &old.replace("sha256", "sha1"),
            &format!("{old}$"),
            &old[..old.len() - 1],
            &old.replace(key, "!!"),
            &old.replace("i=1000", "i=4294967295"),
            &old.replace("i=1000", "i=10000001"),
        ] {
            assert!(!verify("pw", bad), "{bad}");
        }
        assert!(outdated(&old) && outdated("") && outdated("$pbkdf2-sha256$i=x$"));
        assert!(!outdated(&h));
    }

    /// No user, or a hash that is not one, costs what a real check does,
    /// so the time a sign-in takes does not say which names exist.
    #[test]
    fn no_user_takes_as_long_as_a_wrong_password() {
        let h = block(hash("correct horse")).unwrap();
        let time = |hash: Option<&str>| {
            let t = Instant::now();
            assert!(!block(check("wrong", hash)).unwrap());
            t.elapsed()
        };
        let (real, none, junk) = (time(Some(&h)), time(None), time(Some("x")));
        for other in [none, junk] {
            // Loose: other tests hash on the same threads meanwhile.
            assert!(
                other * 4 > real && other < real * 4,
                "{real:?} vs {other:?}"
            );
        }
    }

    /// The queue is counted in rounds: hashes planted with many cannot
    /// queue more work than ordinary ones, yet one always runs.
    #[test]
    fn the_queue_is_counted_in_rounds() {
        let (r, max) = (u64::from(ROUNDS), u64::from(MAX_ROUNDS));
        let budget = QUEUED as u64 * r;
        let mut queued = 0;
        let admitted = (0..QUEUED)
            .filter(|_| {
                admits(queued, r, budget) && {
                    queued += r;
                    true
                }
            })
            .count();
        assert_eq!(admitted, QUEUED);
        assert!(!admits(queued, r, budget));
        assert!(admits(0, max, budget) && !admits(max, max, budget));
    }

    /// A hash leaves its worker free: a request beside it on the same
    /// single-threaded runtime is answered long before the hash is done.
    #[test]
    fn hashing_leaves_the_worker_free() {
        let start = Instant::now();
        let (h, hashed, answered) = block(async {
            let hashing = tokio::spawn(async { (hash("correct horse").await, Instant::now()) });
            tokio::time::sleep(Duration::from_millis(1)).await;
            let answered = Instant::now();
            let (h, hashed) = hashing.await.unwrap();
            (h.unwrap(), hashed, answered)
        });
        assert!(answered < hashed, "answered after the hash");
        assert!(
            answered - start < Duration::from_millis(100),
            "{:?}",
            answered - start
        );
        assert!(verify("correct horse", &h));
    }

    /// A panic in work sent off comes back to the caller, and the threads
    /// go on.
    #[test]
    fn panics_come_back() {
        let caught = std::panic::catch_unwind(|| block(off_worker(1, || panic!("boom"))));
        assert!(caught.is_err());
        assert_eq!(block(off_worker(1, || 7)).unwrap(), 7);
    }
}
