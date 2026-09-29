use wisp::prelude::*;

/// The edge build's outbound fetch: `/relay?url=...` answers with the
/// status and body `url` answered with. A 404 in a native build.
pub async fn get(cx: &mut Cx) -> Result<Response> {
    let url = cx.query("url").or_400()?.into_owned();
    relay(url).await
}

#[cfg(target_arch = "wasm32")]
async fn relay(url: String) -> Result<Response> {
    let reply = wisp::edge::fetch(wisp::Request::new("GET", &url)).await?;
    Ok(Response::text(format!("{} {}", reply.status, reply.text())))
}

#[cfg(not(target_arch = "wasm32"))]
async fn relay(_url: String) -> Result<Response> {
    Err(error(404, "Only the edge build fetches"))
}
