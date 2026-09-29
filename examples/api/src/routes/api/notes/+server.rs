use notes::Note;

/// A note to add, as JSON: `{"title": "Buy tea", "tags": ["home"]}`.
#[derive(FromJson)]
struct NewNote {
    #[validate(min_len = 1, max_len = 200)]
    title: String,
    #[validate(max_len = 10)]
    tags: Option<Vec<String>>,
}

/// Every note, or those whose title has `q` in it: `/api/notes?q=tea`.
fn get(q: Option<String>) -> Vec<Note> {
    notes::list(q.as_deref())
}

fn post(body: NewNote) -> Response {
    let note = notes::add(body.title, body.tags.unwrap_or_default());
    wisp::channel("notes").send(wisp::to_json(&note));
    Response::created(&note)
}
