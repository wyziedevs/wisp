/// The app's components, as files, to check what `file_in` serves.
async fn get(name: String) -> Result<Response> {
    Response::file_in(concat!(env!("CARGO_MANIFEST_DIR"), "/src/components"), &name).await
}
