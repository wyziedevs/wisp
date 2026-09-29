/// Each note as it is added or changed, as server-sent events: in a page,
/// `new EventSource("/api/events")`; in a terminal, `curl -N`.
fn get() -> Response {
    // Subscribed before answering, so nothing sent from now on is missed.
    let mut changes = wisp::channel("notes").subscribe();
    Response::events(|events| async move {
        while let Some(note) = changes.recv().await {
            events.event(&note).await?;
        }
        Ok(())
    })
}
