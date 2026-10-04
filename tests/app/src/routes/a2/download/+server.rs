/// A file the browser saves: a link to it leaves the page where it is.
fn get() -> Response {
    Response::download("note.txt", "hi\n")
}
