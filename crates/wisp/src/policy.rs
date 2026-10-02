//! What a connection may cost, the same for every way the server runs it:
//! tokio's sockets, the epoll, the ring, and the epoll driver answering by
//! itself (`http::on_driver`). How long it may take to send a request or to
//! take a response, whether it stays open, and which buffers it keeps.
//!
//! Plain functions of the clock (`http::seconds()`, passed in) and of sizes,
//! inlined where they are used: no state, no timer, no call. Tested here on
//! a made-up clock. The edge build, which has no connections, keeps only
//! what its answers use.

use std::time::Duration;

/// Time allowed to receive a request's head once its first byte arrived,
/// and then for each part of its body: a large upload may take minutes, as
/// long as it keeps coming.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Bytes a second a request body must average, after `REQUEST_TIMEOUT`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const MIN_BODY_RATE: usize = 1024;
/// Time an idle keep-alive connection is kept open.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// Time a client may take none of a response before it is dropped.
pub(crate) const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// A buffer that grew past this for one large message is shrunk afterwards,
/// so memory per idle connection stays bounded.
pub(crate) const KEEP_CAPACITY: usize = 64 * 1024;
/// What a read buffer that grew past `KEEP_CAPACITY` shrinks to.
pub(crate) const READ_CAPACITY: usize = 8 * 1024;

/// When a connection waiting for its next request at `now` is closed.
#[cfg(not(target_arch = "wasm32"))]
#[inline]
pub(crate) fn idle_deadline(now: u64) -> u64 {
    now + IDLE_TIMEOUT.as_secs()
}

/// When a request whose head started coming at `since` must have all of it.
#[cfg(not(target_arch = "wasm32"))]
#[inline]
pub(crate) fn head_deadline(since: u64) -> u64 {
    since + REQUEST_TIMEOUT.as_secs()
}

/// When the next part of a body that started coming at `since`, with
/// `received` bytes of the request in, must come by, at `now`. Each part in
/// time, and the whole at `MIN_BODY_RATE` at least after the same grace: a
/// body sent a byte at a time cannot hold a connection for days.
#[cfg(not(target_arch = "wasm32"))]
#[inline]
pub(crate) fn body_deadline(now: u64, since: u64, received: usize) -> u64 {
    let rate = since + REQUEST_TIMEOUT.as_secs() + (received / MIN_BODY_RATE) as u64;
    (now + REQUEST_TIMEOUT.as_secs()).min(rate)
}

/// A send that last made progress at `since` has stalled by `now`: its
/// client stopped taking the response, and is dropped. (Tokio's sockets
/// time a write instead: `http::write`.)
#[cfg(target_os = "linux")]
#[inline]
pub(crate) fn stalled(now: u64, since: u64) -> bool {
    now >= since + WRITE_TIMEOUT.as_secs()
}

/// Whether a connection stays open after a request that `asked` it to,
/// while the server is `stopping` or not.
#[cfg(not(target_arch = "wasm32"))]
#[inline]
pub(crate) fn keeps_open(asked: bool, stopping: bool) -> bool {
    asked && !stopping
}

/// Whether to answer `100 Continue` now: the request `expects` it (only
/// HTTP/1.1 can, see `http::parse`), and it was not `sent` for it yet.
#[cfg(not(target_arch = "wasm32"))]
#[inline]
pub(crate) fn continues(expects: bool, sent: bool) -> bool {
    expects && !sent
}

/// Whether a buffer of `capacity`, now empty, is worth keeping as it is.
#[inline]
pub(crate) fn kept(capacity: usize) -> bool {
    capacity <= KEEP_CAPACITY
}

/// Shrinks `buf` to `to` when one large message grew it past keeping.
#[cfg(not(target_arch = "wasm32"))]
#[inline]
pub(crate) fn trim(buf: &mut Vec<u8>, to: usize) {
    if !kept(buf.capacity()) {
        buf.shrink_to(to);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_idle_connection_has_a_minute() {
        assert_eq!(idle_deadline(1000), 1060);
    }

    #[test]
    fn a_head_has_ten_seconds_from_its_first_byte() {
        // However late it is asked: the head's own start counts.
        assert_eq!(head_deadline(1000), 1010);
    }

    #[test]
    fn a_body_comes_in_parts_in_time_and_at_the_least_rate() {
        // Started at 0: the first part within 10 s.
        assert_eq!(body_deadline(0, 0, 0), 10);
        // At 5 s with nothing more: still 10, not 15.
        assert_eq!(body_deadline(5, 0, 0), 10);
        // Fast enough: each part may take 10 s.
        assert_eq!(body_deadline(20, 0, 1 << 20), 30);
        // A byte every 9 s, each in time: the rate ends it. At 1 KB a second
        // it would have its 10 s of grace plus a second a KB.
        let (mut now, mut received) = (0, 0);
        while now < body_deadline(now, 0, received) {
            (now, received) = (now + 9, received + 1);
        }
        assert!(now <= 20, "{now}");
        let fast = body_deadline(30, 0, 30 * MIN_BODY_RATE);
        assert_eq!(fast, 40);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_send_stalls_after_thirty_seconds_without_progress() {
        assert!(!stalled(129, 100));
        assert!(stalled(130, 100));
    }

    #[test]
    fn a_connection_stays_open_unless_asked_or_stopping() {
        assert!(keeps_open(true, false));
        assert!(!keeps_open(false, false));
        assert!(!keeps_open(true, true));
    }

    #[test]
    fn continue_is_sent_once_and_only_when_expected() {
        assert!(continues(true, false));
        assert!(!continues(true, true));
        assert!(!continues(false, false));
    }

    #[test]
    fn buffers_past_keeping_shrink() {
        let mut big = Vec::<u8>::with_capacity(KEEP_CAPACITY + 1);
        trim(&mut big, READ_CAPACITY);
        assert!(big.capacity() < KEEP_CAPACITY);
        let mut fine = Vec::<u8>::with_capacity(KEEP_CAPACITY);
        trim(&mut fine, READ_CAPACITY);
        assert_eq!(fine.capacity(), KEEP_CAPACITY);
        assert!(kept(KEEP_CAPACITY) && !kept(KEEP_CAPACITY + 1));
    }
}
