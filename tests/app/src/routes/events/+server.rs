use wisp::prelude::*;

/// Three events, then the stream ends.
pub fn get() -> Response {
    let (res, events) = Response::events();
    tokio::spawn(async move {
        for i in 0..3 {
            if events.event(&format!("tick {i}\nline two")).await.is_err() {
                return; // the client left
            }
        }
    });
    res
}
