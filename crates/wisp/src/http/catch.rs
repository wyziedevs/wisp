//! Handler panics: caught as a 500, reported once, with where they were.

use super::*;

thread_local! {
    /// Set while `catch` polls a handler. A panic there is reported once, as
    /// the request's 500, rather than by the panic hook as well.
    static IN_HANDLER: Cell<bool> = const { Cell::new(false) };
    /// Where the handler that just panicked was, for the 500's message.
    static PANICKED_AT: Cell<Option<String>> = const { Cell::new(None) };
    /// Dev builds: the longest a handler of this request held the thread in
    /// one poll.
    pub(super) static BLOCKED: Cell<Duration> = const { Cell::new(Duration::ZERO) };
}

/// Quiets the panic hook for handler panics, which `catch` reports with the
/// request, and remembers where each happened. Any other panic, or any at
/// all with `RUST_BACKTRACE` set, still reaches the hook that was there.
pub(super) fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if IN_HANDLER.get() {
                if let Some(l) = info.location() {
                    PANICKED_AT.set(Some(format!(
                        "{}:{}:{}",
                        short_path(l.file()),
                        l.line(),
                        l.column()
                    )));
                }
                if std::env::var_os("RUST_BACKTRACE").is_none() {
                    return;
                }
            }
            previous(info);
        }));
    });
}

/// `file` relative to the working directory, with `/` separators, when it is
/// inside it: `src/routes/+page.rs` rather than the whole path the
/// generated code gave the compiler.
pub(super) fn short_path(file: &str) -> String {
    let rel = std::env::current_dir()
        .ok()
        .and_then(|d| Path::new(file).strip_prefix(d).ok().map(Path::to_path_buf));
    match rel {
        Some(r) => r.to_string_lossy().replace('\\', "/"),
        None => file.to_string(),
    }
}

/// Runs a handler future, turning a panic into a 500 so one bad request
/// cannot take the connection (or anything else) down with it.
pub(crate) async fn catch<F: Future<Output = crate::Result<()>>>(f: F) -> crate::Result<()> {
    catch_made(|| f).await
}

/// [`catch`] of the future `make` makes. The future is made in place: a
/// future taken as an argument would be held twice over, as it came and
/// as it is polled, and moved in full each request.
pub(super) async fn catch_made<F: Future<Output = crate::Result<()>>>(
    make: impl FnOnce() -> F,
) -> crate::Result<()> {
    let mut f = std::pin::pin!(make());
    let settings = crate::settings();
    let (timed, mut late) = (
        settings.timed,
        crate::timeout::Late::within(settings.timeout_ms),
    );
    // The request's span, current while it is polled.
    let trace = crate::otel::adopt();
    std::future::poll_fn(move |cx| {
        let began = timed.then(Instant::now);
        let was = trace.map(crate::otel::enter);
        IN_HANDLER.set(true);
        let polled = catch_unwind(AssertUnwindSafe(|| f.as_mut().poll(cx)));
        IN_HANDLER.set(false);
        if let Some(was) = was {
            crate::otel::leave(was);
        }
        if let Some(began) = began {
            BLOCKED.set(BLOCKED.get().max(began.elapsed()));
        }
        let polled = polled.unwrap_or_else(|panic| Poll::Ready(Err(panicked(panic))));
        if polled.is_pending() && late.over(cx) {
            return Poll::Ready(Err(crate::timeout::error()));
        }
        polled
    })
    .await
}

/// [`catch`] of a sync [`App::handle_now`]: `None` when it answered
/// nothing.
#[cfg(target_os = "linux")]
pub(super) fn catch_now(
    timed: bool,
    f: impl FnOnce() -> crate::Result<bool>,
) -> Option<crate::Result<()>> {
    let began = timed.then(Instant::now);
    IN_HANDLER.set(true);
    let ran = catch_unwind(AssertUnwindSafe(f));
    IN_HANDLER.set(false);
    if let Some(began) = began {
        BLOCKED.set(BLOCKED.get().max(began.elapsed()));
    }
    match ran {
        Ok(Ok(true)) => Some(Ok(())),
        Ok(Ok(false)) => None,
        Ok(Err(e)) => Some(Err(e)),
        Err(panic) => Some(Err(panicked(panic))),
    }
}

/// The 500 of a handler's `panic`, saying where it was.
pub(super) fn panicked(panic: Box<dyn std::any::Any + Send>) -> Error {
    let msg = panic
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".into());
    let msg = match PANICKED_AT.take() {
        Some(at) => format!("panic at {at}: {msg}"),
        None => format!("panic: {msg}"),
    };
    Error::new(500, msg)
}
