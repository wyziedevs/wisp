use wisp::prelude::*;

/// The app's components, as files, to check what `file_in` serves.
pub async fn get(cx: &mut Cx) -> Result<Response> {
    Response::file_in(concat!(env!("CARGO_MANIFEST_DIR"), "/src/components"), cx.param("name")).await
}
