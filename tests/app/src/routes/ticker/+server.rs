use std::time::Duration;

/// An event every 20 ms for ten seconds: a stream that stays open.
fn get() -> Response {
    Response::events(|events| async move {
        for i in 0..500 {
            events.event(&format!("tick {i}")).await?;
            wisp::sleep(Duration::from_millis(20)).await;
        }
        Ok(())
    })
}
