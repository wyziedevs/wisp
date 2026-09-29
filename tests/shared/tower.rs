//! What the tower tests of `wisp` and of the test app share.

use std::future::poll_fn;
use std::pin::pin;
use wisp::tower::Body;
use wisp::tower::http_body::Body as _;

/// Runs `f` to its end on a runtime of its own.
pub fn run<T>(f: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

/// All of `body`, as text.
pub async fn text(body: Body) -> String {
    let mut body = pin!(body);
    let mut all = Vec::new();
    while let Some(frame) = poll_fn(|cx| body.as_mut().poll_frame(cx)).await {
        all.extend_from_slice(&frame.unwrap().into_data().unwrap());
    }
    String::from_utf8(all).unwrap()
}
