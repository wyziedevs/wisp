use wisp::prelude::*;

/// Every message comes back as it was sent.
pub fn get() -> Response {
    Response::websocket(|ws| async move {
        while let Some(msg) = ws.recv().await {
            ws.send(msg).await?;
        }
        Ok(())
    })
}
