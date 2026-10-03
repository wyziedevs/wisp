//! `/api/notes` and `/api/notes/[id]`: a whole JSON API from the type, saved
//! in `WISP_DATA` so it survives restarts. Try `/api/notes?done=false`,
//! `?title.has=tea`, `?sort=-created_at&limit=20`. Writes need the API key
//! (src/hooks.rs).

#[derive(Rest)]
struct Note {
    #[validate(len = 1..=200)]
    title: String,
    #[validate(max_len = 10)]
    tags: Vec<String>,
    done: bool,
    created_at: String,
}

/// Tells whoever listens (`/api/events`) that a note was added or changed.
fn after_create(note: &Row<Note>) {
    wisp::channel("notes").send(wisp::to_json(note));
}

fn after_update(note: &Row<Note>) {
    wisp::channel("notes").send(wisp::to_json(note));
}
