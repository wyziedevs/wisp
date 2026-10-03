//! `{#await future}` in a page. The page goes out whole, each await's
//! pending markup in place; the response stays open and each answer
//! follows as it comes, after the page (see `protocol::AWAIT_JS`). Only a
//! page with an `{#await}` calls any of this: the build decides, so every
//! other page is answered as it always was.

use crate::{App, Cx, Gone, Out, Response, Sender};
use std::borrow::Cow;
use std::cell::Cell;
use std::fmt::Display;
use std::future::{Future, poll_fn};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::{Pin, pin};
use std::task::{Context, Poll};
use wisp_shared::protocol::{AWAIT_ANSWER, AWAIT_JS, AWAIT_OPEN};

/// An await's answer, made: its HTML, framed for the page.
pub(crate) type Tail = Pin<Box<dyn Future<Output = String> + Send>>;

/// What an `{#await}` with no `{:catch}` shows when its future fails.
const FAILED: &str = "<p role=\"alert\">Something went wrong</p>";

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

/// Forgets what a render that failed part way left (a page starts with
/// none).
pub fn begin(out: &mut Out) {
    out.tails.clear();
}

/// `{#await future}`: opens its pending markup, and keeps `then`, which
/// renders the answer, `None` if the future panicked, once it comes.
pub fn defer<F, T>(out: &mut Out, future: F, then: T)
where
    F: Future + Send + 'static,
    T: FnOnce(&mut Out, Option<F::Output>) + Send + 'static,
{
    let (k, lang) = (out.tails.len(), out.lang);
    out.body.push_str(AWAIT_OPEN);
    out.body.push_str(&k.to_string());
    out.body.push_str("\">");
    out.tails.push(Box::pin(async move {
        let v = Unwind(Box::pin(future)).await;
        let mut o = Out {
            lang,
            ..Out::default()
        };
        let done = catch_unwind(AssertUnwindSafe(|| then(&mut o, v))).is_ok();
        let html = if done { o.body.as_str() } else { FAILED };
        let k = k.to_string();
        [
            AWAIT_ANSWER,
            &k,
            "\">",
            html,
            "</div><script>",
            AWAIT_JS,
            "</script>",
        ]
        .concat()
    }));
}

/// After the page rendered: with awaits, it goes out as the first chunk of
/// a streamed response, their answers after it; with none (each in a
/// branch not taken), as any page does.
pub fn finish<A: App>(cx: &Cx, out: &mut Out) {
    if out.tails.is_empty() {
        return;
    }
    let tails = std::mem::take(&mut out.tails);
    let page = crate::http::page::<A>(out).concat();
    let mut res = Response::stream("text/html; charset=utf-8", move |tx| async move {
        tx.send(page).await?;
        answers(tails, &tx).await
    });
    res.status = cx.status();
    res.page = true;
    if let Some(policy) = crate::csp::header() {
        res.headers
            .push((Cow::Borrowed("content-security-policy"), policy.into()));
    }
    out.response = Some(res);
}

/// Sends each answer as it comes, until all have, or the client is gone.
async fn answers(mut tails: Vec<Tail>, tx: &Sender) -> Result<(), Gone> {
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
            Some(html) => tx.send(html).await?,
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
    pub fn new(v: T) -> Settled<T> {
        Settled(Cell::new(Some(v)))
    }

    fn take(&self) -> Result<T, String> {
        self.0.take().ok_or_else(failed)
    }
}

pub trait WispResult {
    type Value;
    fn settle(&self) -> Result<Self::Value, String>;
}

impl<T> WispResult for &&Settled<crate::Result<T>> {
    type Value = T;
    fn settle(&self) -> Result<T, String> {
        self.take()?.map_err(|e| e.message().to_string())
    }
}

pub trait AnyResult {
    type Value;
    fn settle(&self) -> Result<Self::Value, String>;
}

impl<T, E: Display> AnyResult for &Settled<Result<T, E>> {
    type Value = T;
    fn settle(&self) -> Result<T, String> {
        self.take()?.map_err(|e| e.to_string())
    }
}

pub trait Value {
    type Value;
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

/// The error a `{:catch e}` gets when the future panicked.
pub fn failed() -> String {
    "Something went wrong".into()
}
