/// One event, then nothing ever again: only the client going (or the server
/// stopping) ends it, never a failed write.
fn get() -> Response {
    Response::events(|events| async move {
        events.event("hi").await?;
        std::future::pending::<()>().await;
        Ok(())
    })
}
