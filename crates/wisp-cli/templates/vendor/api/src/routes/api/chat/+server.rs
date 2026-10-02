/// A chat room over a WebSocket: what one client sends, every client in the
/// room gets. In a page, `new WebSocket("ws://" + location.host + "/api/chat")`.
fn get() -> Response {
    wisp::channel("chat").websocket()
}
