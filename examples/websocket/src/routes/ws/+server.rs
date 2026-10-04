/// Every message comes back, numbered, until the client closes.
fn get() -> Response {
    Response::websocket(|ws| async move {
        let mut sent = 0;
        while let Some(msg) = ws.recv().await {
            sent += 1;
            ws.send(format!("{sent}: {}", msg.text())).await?;
        }
        Ok(())
    })
}
