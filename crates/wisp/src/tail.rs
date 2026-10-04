//! `{#await future}` in a page. The page goes out whole, each await's
//! pending markup in place; the response stays open and each answer
//! follows as it comes, after the page (see `protocol::AWAIT_JS`). Only a
//! page with an `{#await}` calls any of this, decided at build: no other
//! page, nor `Out`, holds anything of it.
//!
//! The answers a render defers wait in a thread-local list from its
//! `defer`s to its `finish`: a page's markup renders without awaiting, so
//! no other request's render comes between them on the thread. [`Awaits`],
//! which each such page holds, empties the list when it ends, so a render
//! that panicked part way leaves nothing behind for the next.

use crate::compress::Stream;
use crate::live::{Live, Start};
use crate::{App, Cx, Gone, Out, Response, Sender};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::fmt::Display;
use std::future::{Future, poll_fn};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};
use std::task::{Context, Poll};
use wisp_shared::protocol::{AWAIT_ANSWER, AWAIT_JS, AWAIT_LIVE_OPEN, AWAIT_OPEN};

/// An await's answer, made: its HTML, framed for the page.
type Tail = Pin<Box<dyn Future<Output = String> + Send>>;

/// What an `{#await}` with no `{:catch}` shows when its future fails.
const FAILED: &str = "<p role=\"alert\">Something went wrong</p>";

/// The render's deferred answers, and the number its answers' browser
/// code instances start from (the page's count, once it has rendered).
#[derive(Default)]
struct Pending {
    tails: Vec<Tail>,
    ids: Option<Arc<AtomicU32>>,
}

thread_local! {
    static PENDING: RefCell<Pending> = RefCell::default();
}

/// Held by a page with `{#await}` while it is served: `finish` sends what
/// its render deferred; dropped, it forgets what is left.
pub struct Awaits(());

impl Awaits {
    /// Starts holding the deferred parts of a page.
    pub fn begin() -> Awaits {
        Awaits(())
    }

    /// After the page rendered: with answers to come, it goes out as the
    /// first chunk of a streamed response, the answers after it; with none
    /// (each await in a branch not taken), as any page does.
    pub fn finish<A: App>(self, cx: &Cx, out: &mut Out) {
        let Pending { tails, ids } = PENDING.with_borrow_mut(std::mem::take);
        if tails.is_empty() {
            return;
        }
        if let Some(ids) = ids {
            ids.store(out.live.count(), Relaxed);
        }
        let page = crate::http::page::<A>(out).concat();
        let gzip = crate::compress::wanted(cx);
        let mut res = Response::stream("text/html; charset=utf-8", move |tx| async move {
            let mut pipe = Pipe {
                tx: &tx,
                z: gzip.then(Stream::new),
            };
            pipe.send(page.as_bytes()).await?;
            answers(tails, &mut pipe).await?;
            pipe.end().await
        });
        res.status = cx.status();
        res.page = true;
        let h = &mut res.headers;
        if let Some(policy) = crate::csp::header() {
            h.push((Cow::Borrowed("content-security-policy"), policy.into()));
        }
        // As a compressed file is: the answer depends on `accept-encoding`.
        h.push((Cow::Borrowed("vary"), "accept-encoding".into()));
        if gzip {
            h.push((Cow::Borrowed("content-encoding"), "gzip".into()));
        }
        out.response = Some(res);
    }
}

impl Drop for Awaits {
    fn drop(&mut self) {
        PENDING.with_borrow_mut(|p| *p = Pending::default());
    }
}

/// Where a streamed page goes: to the client, a gzip piece at a time when
/// it takes gzip, each flushed (see `compress::Stream`).
struct Pipe<'a> {
    tx: &'a Sender,
    z: Option<Stream>,
}

impl Pipe<'_> {
    async fn send(&mut self, s: &[u8]) -> Result<(), Gone> {
        match self.z.as_mut() {
            Some(z) => self.tx.send(z.piece(s)).await,
            None => self.tx.send(s).await,
        }
    }

    async fn end(self) -> Result<(), Gone> {
        match self.z {
            Some(z) => self.tx.send(z.end()).await,
            None => Ok(()),
        }
    }
}

/// A future whose panic is `None`, so one await cannot take the response
/// (or the worker) down. Polled no more once it is ready.
struct Unwind<F>(Pin<Box<F>>);

impl<F: Future> Future for Unwind<F> {
    type Output = Option<F::Output>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match catch_unwind(AssertUnwindSafe(|| self.0.as_mut().poll(cx))) {
            Ok(Poll::Ready(v)) => Poll::Ready(Some(v)),
            Ok(Poll::Pending) => Poll::Pending,
            Err(_) => Poll::Ready(None),
        }
    }
}

/// `future`, or `None` once `WISP_HANDLER_TIMEOUT` has passed: an answer
/// waits no longer than a handler may.
async fn within<F: Future>(future: F) -> Option<F::Output> {
    let ms = crate::settings().timeout_ms;
    #[cfg(not(target_arch = "wasm32"))]
    if ms > 0 {
        let ms = std::time::Duration::from_millis(ms);
        return tokio::time::timeout(ms, future).await.ok();
    }
    let _ = ms;
    Some(future.await)
}

/// `{#await future}`: opens its pending markup, and keeps `then`, which
/// renders the answer, `None` if the future failed to finish (it panicked,
/// or took too long), once it comes.
pub fn defer<F, T>(out: &mut Out, future: F, then: T)
where
    F: Future + Send + 'static,
    T: FnOnce(&mut Out, Option<F::Output>) + Send + 'static,
{
    let (lang, around) = (out.lang, out.live.around());
    let (k, ids) = PENDING.with_borrow_mut(|p| {
        let ids = p.ids.get_or_insert_with(Default::default).clone();
        (p.tails.len(), ids)
    });
    out.body.push_str(AWAIT_OPEN);
    out.body.push_str(&k.to_string());
    out.body.push_str("\">");
    let tail: Tail = Box::pin(async move {
        let v = within(Unwind(Box::pin(future))).await.flatten();
        answer(k, lang, around, &ids, |o| then(o, v))
    });
    PENDING.with_borrow_mut(|p| p.tails.push(tail));
}

/// The answer of await `k`, as `render` writes it, framed: its instances
/// are numbered on from `ids`, which it moves on.
fn answer(
    k: usize,
    lang: u8,
    around: Option<(u32, Start)>,
    ids: &AtomicU32,
    render: impl FnOnce(&mut Out),
) -> String {
    let mut o = Out {
        lang,
        live: Live::after(ids.load(Relaxed), around),
        ..Out::default()
    };
    let done = catch_unwind(AssertUnwindSafe(|| render(&mut o))).is_ok();
    let mut s = format!("{AWAIT_ANSWER}{k}\">");
    if done {
        ids.store(o.live.count(), Relaxed);
        s.push_str(&o.body);
        o.live.write(&mut s, lang, AWAIT_LIVE_OPEN);
    } else {
        s.push_str(FAILED);
    }
    for part in ["</div><script>", AWAIT_JS, "</script>"] {
        s.push_str(part);
    }
    s
}

/// Sends each answer as it comes, until all have, or the client is gone.
async fn answers(mut tails: Vec<Tail>, pipe: &mut Pipe<'_>) -> Result<(), Gone> {
    let tx = pipe.tx;
    let mut gone = pin!(tx.0.closed());
    while !tails.is_empty() {
        let next = poll_fn(|cx| {
            if gone.as_mut().poll(cx).is_ready() {
                return Poll::Ready(None);
            }
            for k in 0..tails.len() {
                if let Poll::Ready(html) = tails[k].as_mut().poll(cx) {
                    drop(tails.swap_remove(k));
                    return Poll::Ready(Some(html));
                }
            }
            Poll::Pending
        });
        match next.await {
            Some(html) => pipe.send(html.as_bytes()).await?,
            None => return Err(Gone),
        }
    }
    Ok(())
}

/// What an await's future gave, for its branches: a `Result`'s value, or
/// its error as text (an [`Error`](crate::Error)'s message), any other value
/// as it is. The build calls `(&&&Settled::new(v)).settle()`, and rustc
/// picks the impl.
pub struct Settled<T>(Cell<Option<T>>);

impl<T> Settled<T> {
    /// Wraps the value the build is about to settle.
    pub fn new(v: T) -> Settled<T> {
        Settled(Cell::new(Some(v)))
    }

    fn take(&self) -> Result<T, String> {
        self.0.take().ok_or_else(failed)
    }
}

/// Settles a `Result<T, wisp::Error>`: the value, or the error's message.
pub trait WispResult {
    /// What a success holds.
    type Value;
    /// The value, or the error as text.
    fn settle(&self) -> Result<Self::Value, String>;
}

impl<T> WispResult for &&Settled<crate::Result<T>> {
    type Value = T;
    fn settle(&self) -> Result<T, String> {
        self.take()?.map_err(|e| e.message().to_string())
    }
}

/// Settles any other `Result<T, E: Display>`: the value, or the error as text.
pub trait AnyResult {
    /// What a success holds.
    type Value;
    /// The value, or the error as text.
    fn settle(&self) -> Result<Self::Value, String>;
}

impl<T, E: Display> AnyResult for &Settled<Result<T, E>> {
    type Value = T;
    fn settle(&self) -> Result<T, String> {
        self.take()?.map_err(|e| e.to_string())
    }
}

/// Settles any value that is not a `Result`: always the value.
pub trait Value {
    /// The value's type.
    type Value;
    /// Always `Ok` with the value.
    fn settle(&self) -> Result<Self::Value, String>;
}

impl<T> Value for Settled<T> {
    type Value = T;
    fn settle(&self) -> Result<T, String> {
        self.take()
    }
}

/// What an `{#await}` with no `{:catch}` shows when it fails.
pub fn failed_html(out: &mut Out) {
    out.body.push_str(FAILED);
}

/// The error a `{:catch e}` gets when the future did not finish.
pub fn failed() -> String {
    "Something went wrong".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A render that stopped before `finish` (it panicked, or its request
    /// was dropped) leaves nothing for the next page on the thread.
    #[test]
    fn a_render_that_stopped_leaves_nothing() {
        let mut out = Out::default();
        {
            let _awaits = Awaits::begin();
            defer(&mut out, async {}, |_, _| {});
            PENDING.with_borrow(|p| assert_eq!(p.tails.len(), 1));
        }
        PENDING.with_borrow(|p| assert!(p.tails.is_empty() && p.ids.is_none()));
        assert_eq!(out.body, "<wisp-await id=\"wisp-await-0\">");
    }
}
