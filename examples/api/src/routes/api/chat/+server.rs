/// A chat room over a WebSocket: what one client sends, every client in the
/// room gets. In a page, `new WebSocket("ws://" + location.host + "/api/chat")`.
fn get() -> Response {
    Response::websocket(|ws| async move {
        wisp::channel("chat").connect(&ws).await?;
        Ok(())
    })
}
