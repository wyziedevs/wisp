
/// Three events, a moment apart, then the stream ends.
fn get() -> Response {
    Response::events(|events| async move {
        for i in 0..3 {
            events.event(&format!("tick {i}\nline two")).await?;
            wisp::sleep(Duration::from_millis(5)).await;
        }
        Ok(())
    })
}
