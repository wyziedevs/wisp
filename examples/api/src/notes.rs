//! The notes, kept in memory. A real app keeps them in a database: it opens
//! a pool in `init` (src/hooks.rs), hands it to `wisp::provide`, and reads
//! it here with `wisp::state`. docs/api.md shows sqlx, rusqlite and Redis.
//!
//! A file in src/ is a module of the app with no `mod` line: routes call
//! these as `notes::list`, and Wisp's prelude is in scope, as in a route.

#[derive(Clone, Json)]
pub struct Note {
    pub id: u64,
    pub title: String,
    pub tags: Vec<String>,
    pub done: bool,
}

/// The last id given out, and the notes.
static NOTES: Shared<(u64, Vec<Note>)> = Shared::new((0, Vec::new()));

fn notes() -> std::sync::MutexGuard<'static, (u64, Vec<Note>)> {
    NOTES.lock()
}

/// Every note, or those whose title has `q` in it.
pub fn list(q: Option<&str>) -> Vec<Note> {
    let q = q.unwrap_or("").to_lowercase();
    notes()
        .1
        .iter()
        .filter(|n| n.title.to_lowercase().contains(&q))
        .cloned()
        .collect()
}

pub fn find(id: u64) -> Option<Note> {
    notes().1.iter().find(|n| n.id == id).cloned()
}

pub fn add(title: String, tags: Vec<String>) -> Note {
    let mut all = notes();
    all.0 += 1;
    let note = Note {
        id: all.0,
        title,
        tags,
        done: false,
    };
    all.1.push(note.clone());
    note
}

/// Changes what is `Some`; `None` if there is no such note.
pub fn change(id: u64, title: Option<String>, done: Option<bool>) -> Option<Note> {
    let mut all = notes();
    let note = all.1.iter_mut().find(|n| n.id == id)?;
    if let Some(title) = title {
        note.title = title;
    }
    if let Some(done) = done {
        note.done = done;
    }
    Some(note.clone())
}

/// Whether there was such a note.
pub fn remove(id: u64) -> bool {
    let mut all = notes();
    let before = all.1.len();
    all.1.retain(|n| n.id != id);
    all.1.len() < before
}
