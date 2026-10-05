// GET /api/notes
fn list() -> Vec<Note> {
    db::all()
}
fn post(body: New) -> Response {
    Response::created(&db::add(body))
}
// `id` → /api/notes/[id]
fn get(id: u64) -> Option<Note> {
    db::find(id)
}
