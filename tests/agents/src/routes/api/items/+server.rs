use crate::db::{New, Note};

fn list() -> Vec<Note> { db::all() }                  // GET /api/notes
fn post(body: New) -> Response { Response::created(&db::add(body)) }
fn get(id: u64) -> Option<Note> { db::find(id) }      // an `id` param → /api/notes/[id]
fn delete(id: u64) -> Option<()> { db::remove(id) }   // None → 404, Some(()) → 204
fn before(cx: &mut Cx) -> Result {                    // runs before each handler here
    if cx.writes() { cx.need_bearer("API_KEY")?; }    // 401 unless Bearer $API_KEY
    Ok(())
}
