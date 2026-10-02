//! Passwords, kept as hashes: `let hash = wisp::password::hash(&password).await;`
//! when one is chosen, `wisp::password::verify(&password, &user.hash).await`
//! when it is typed again.
//!
//! The hash is PBKDF2-HMAC-SHA256 (RFC 8018) with a random 16-byte salt and
//! 600,000 rounds, what OWASP asks of it, written with its rounds in the
//! PHC string form: `$pbkdf2-sha256$i=600000$<salt>$<key>` (base64, no
//! padding). `verify` reads the rounds from the hash, so hashes made with
//! fewer (or, later, more) still check; [`outdated`] says which to make
//! again, from the password, the next time it is typed.
//!
//! A hash takes about a fifth of a second of one core, on purpose: that is
//! what makes guessing slow. It runs on a thread kept for hashing, one for
//! every two cores, so the request's worker goes on answering its other
//! connections meanwhile, and a flood of sign-ins takes at most half the
//! machine. The edge build has no threads and hashes in place.

use crate::sign::{base64, pbkdf2, random, unbase64};

/// The rounds [`hash`] uses.
pub const ROUNDS: u32 = 600_000;

/// The most rounds [`verify`] spends on a hash, about 3 s of a core: a
/// stored hash that names more (damaged, or planted) is `false` rather than
/// a worker thread held for hours.
const MAX_ROUNDS: u32 = 10_000_000;

const PREFIX: &str = "$pbkdf2-sha256$i=";

/// `password`, hashed with a salt of its own, as text to keep.
pub async fn hash(password: &str) -> String {
    let salt = random::<16>();
    let password = password.as_bytes().to_vec();
    let key = off_worker(move || pbkdf2(&password, &salt, ROUNDS)).await;
    encode(ROUNDS, &salt, &key)
}

/// Whether `password` is the one `hash` (made by [`hash`]) was made from,
/// in time that does not depend on where they differ. Any other text is
/// `false`.
pub async fn verify(password: &str, hash: &str) -> bool {
    let Some((rounds, salt, salt_len, key)) = parse(hash) else {
        return false;
    };
    let password = password.as_bytes().to_vec();
    let made = off_worker(move || pbkdf2(&password, &salt[..salt_len], rounds)).await;
    crate::secure_eq(made, key)
}

/// A hash's rounds, salt (and its length) and key; `None` for any text
/// [`hash`] does not make, or one naming more than [`MAX_ROUNDS`].
fn parse(hash: &str) -> Option<(u32, [u8; 64], usize, [u8; 32])> {
    let rest = hash.strip_prefix(PREFIX)?;
    let mut parts = rest.split('$');
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
    Some((rounds, salt_bytes, salt_len, key_bytes))
}

/// `work`'s answer, worked out on one of the threads kept for hashing while
/// the caller's worker serves its other connections. The threads start with
/// the first hash; where none will start, or in the edge build, it runs here.
#[cfg(not(target_arch = "wasm32"))]
async fn off_worker<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
    use std::sync::{Arc, Mutex, OnceLock, mpsc};
    type Job = Box<dyn FnOnce() + Send>;
    static POOL: OnceLock<Option<mpsc::Sender<Job>>> = OnceLock::new();
    let pool = POOL.get_or_init(|| {
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        let (send, take) = mpsc::channel::<Job>();
        let take = Arc::new(Mutex::new(take));
        let mut started = 0;
        for i in 0..cores.div_ceil(2) {
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
        (started > 0).then_some(send)
    });
    let (done, answer) = tokio::sync::oneshot::channel();
    // A panic comes back to the caller, as if the work had run there; the
    // thread goes on.
    let job: Job = Box::new(move || {
        let _ = done.send(catch_unwind(AssertUnwindSafe(work)));
    });
    let unsent = match pool {
        Some(pool) => pool.send(job).err().map(|e| e.0),
        None => Some(job),
    };
    if let Some(job) = unsent {
        job();
    }
    match answer.await {
        Ok(Ok(v)) => v,
        Ok(Err(panic)) => resume_unwind(panic),
        Err(_) => panic!("a password hash was dropped before it ran"),
    }
}

#[cfg(target_arch = "wasm32")]
async fn off_worker<T>(work: impl FnOnce() -> T) -> T {
    work()
}

/// Whether `hash` was made with fewer rounds than [`hash`] uses now, or is
/// not one of its hashes: make it again once the password checks.
///
/// ```ignore
/// if wisp::password::verify(&password, &user.hash) && wisp::password::outdated(&user.hash) {
///     USERS.update(user.id, |u| u.hash = wisp::password::hash(&password));
/// }
/// ```
pub fn outdated(hash: &str) -> bool {
    let rounds = hash.strip_prefix(PREFIX).and_then(|r| r.split('$').next());
    rounds
        .and_then(|r| r.parse::<u32>().ok())
        .is_none_or(|r| r < ROUNDS)
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
        block(super::verify(password, hash))
    }

    #[test]
    fn hashes_check_and_carry_their_rounds() {
        let h = block(hash("correct horse"));
        assert!(h.starts_with("$pbkdf2-sha256$i=600000$"), "{h}");
        assert_eq!(h.len(), PREFIX.len() + 6 + 1 + 22 + 1 + 43);
        assert!(verify("correct horse", &h));
        assert!(!verify("correct horsf", &h));
        assert_ne!(block(hash("correct horse")), h, "salted");

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
            (h, hashed, answered)
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
        let caught = std::panic::catch_unwind(|| block(off_worker(|| panic!("boom"))));
        assert!(caught.is_err());
        assert_eq!(block(off_worker(|| 7)), 7);
    }
}
