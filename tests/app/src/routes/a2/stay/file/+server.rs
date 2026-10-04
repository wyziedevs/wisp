/// A file the browser saves, from a page with a `+loading.wisp`.
fn get() -> Response {
    Response::download("note.txt", "hi\n")
}
