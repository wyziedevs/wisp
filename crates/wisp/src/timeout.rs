#![cfg_attr(target_arch = "wasm32", allow(unused_imports))]
//! `WISP_HANDLER_TIMEOUT` (seconds, unset or 0 for none): a handler that
//! has not finished by then is dropped and the request gets a 503. Checked
//! only when the handler waits, with a timer made then: a handler that
//! answers at once, as most do, pays a read of one number.
//!
//! A handler that holds its thread without awaiting cannot be stopped, as
//! with any async code: that is what the dev build's blocking warning is for.

use crate::Error;
use std::future::Future;
use std::pin::Pin;
use std::task::Context;
use std::time::Duration;

/// The deadline of one handler.
pub(crate) struct Late {
    ms: u64,
    #[cfg(not(target_arch = "wasm32"))]
    timer: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl Late {
    /// A deadline of `ms` milliseconds, 0 for none (`Settings::timeout_ms`).
    pub(crate) fn within(ms: u64) -> Late {
        Late {
            ms,
            #[cfg(not(target_arch = "wasm32"))]
            timer: None,
        }
    }

    /// Whether the handler, which just waited, is out of time. Wakes `cx`
    /// when it will be.
    #[inline]
    pub(crate) fn over(&mut self, cx: &mut Context) -> bool {
        if self.ms == 0 {
            return false;
        }
        self.expired(cx)
    }

    #[cfg(target_arch = "wasm32")]
    fn expired(&mut self, _: &mut Context) -> bool {
        false
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn expired(&mut self, cx: &mut Context) -> bool {
        let ms = self.ms;
        let timer = self
            .timer
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(Duration::from_millis(ms))));
        timer.as_mut().poll(cx).is_ready()
    }
}

/// A route's `const TIMEOUT: u32 = 5;` (seconds): `f` is dropped and the
/// request answered with a 503 if it takes longer. Only routes that set it
/// call this.
#[cfg(not(target_arch = "wasm32"))]
pub async fn within<T>(secs: u32, f: impl Future<Output = crate::Result<T>>) -> crate::Result<T> {
    match tokio::time::timeout(Duration::from_secs(secs.into()), f).await {
        Ok(r) => r,
        Err(_) => Err(error()),
    }
}

/// The edge has no timer: the host's own limit applies.
#[cfg(target_arch = "wasm32")]
pub async fn within<T>(_: u32, f: impl Future<Output = crate::Result<T>>) -> crate::Result<T> {
    f.await
}

/// What the request is answered with.
pub(crate) fn error() -> Error {
    Error::new(503, "The server took too long to answer")
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::task::Poll;

    fn run(late: &mut Late, wait: Duration) -> bool {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut sleep = std::pin::pin!(tokio::time::sleep(wait));
            std::future::poll_fn(|cx| {
                if sleep.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(false); // the handler finished
                }
                if late.over(cx) {
                    return Poll::Ready(true);
                }
                Poll::Pending
            })
            .await
        })
    }

    #[test]
    fn a_slow_handler_is_over() {
        assert!(run(&mut Late::within(30), Duration::from_secs(5)));
    }

    #[test]
    fn a_quick_one_is_not() {
        assert!(!run(&mut Late::within(5000), Duration::from_millis(20)));
    }

    #[test]
    fn no_limit_never_is() {
        let mut late = Late::within(0);
        assert!(!run(&mut late, Duration::from_millis(50)));
        assert!(late.timer.is_none());
    }

    #[test]
    fn it_answers_503() {
        assert_eq!(error().status(), 503);
    }
}
