//! A list streamed as newline-delimited JSON, a row per line.

fn get() -> Response {
    Response::ndjson(|out| async move {
        for n in 1..=3 {
            out.line(&n).await?;
        }
        Ok(())
    })
}
