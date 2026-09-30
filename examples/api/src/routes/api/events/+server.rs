/// Each note as it is added or changed, as server-sent events: in a page,
/// `new EventSource("/api/events")`; in a terminal, `curl -N`.
fn get() -> Response {
    wisp::channel("notes").events()
}
