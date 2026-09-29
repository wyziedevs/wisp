/// The edge build's outbound fetch: `/relay?url=...` answers with the
/// status and body `url` answered with. A 404 in a native build.
async fn get(url: String) -> Result<Response> {
    relay(url).await
}

#[cfg(target_arch = "wasm32")]
async fn relay(url: String) -> Result<Response> {
    let reply = wisp::edge::fetch(wisp::Request::new("GET", &url)).await?;
    Ok(Response::text(format!("{} {}", reply.status, reply.text())))
}

#[cfg(not(target_arch = "wasm32"))]
async fn relay(_url: String) -> Result<Response> {
    error(404, "Only the edge build fetches")
}
