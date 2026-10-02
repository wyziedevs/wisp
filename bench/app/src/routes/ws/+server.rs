// Echoes each message back.

fn get() -> Response {
    Response::websocket(|ws| async move {
        while let Some(msg) = ws.recv().await {
            ws.send(msg).await?;
        }
        Ok(())
    })
}
