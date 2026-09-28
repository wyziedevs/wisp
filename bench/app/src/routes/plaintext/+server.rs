use wisp::prelude::*;

pub async fn get(_cx: &mut Cx) -> Result<Response> {
    Ok(Response::text("Hello, World!"))
}
