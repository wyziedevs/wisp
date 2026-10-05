
/// Events with no end of their own, for the tests of stopping the server and of
/// clients that leave: only the server stopping, or the client going, ends it.
fn get() -> Response {
    Response::events(|events| async move {
        loop {
            events.event("tick").await?;
            wisp::sleep(Duration::from_millis(5)).await;
        }
    })
}
