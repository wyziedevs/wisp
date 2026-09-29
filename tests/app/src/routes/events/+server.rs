use std::time::Duration;
use wisp::prelude::*;

/// Three events, a moment apart, then the stream ends.
pub fn get() -> Response {
    let (res, events) = Response::events();
    wisp::spawn(async move {
        for i in 0..3 {
            if events.event(&format!("tick {i}\nline two")).await.is_err() {
                return; // the client left
            }
            wisp::sleep(Duration::from_millis(5)).await;
        }
    });
    res
}
