//! What AGENTS.md's handlers by hand call (`api/items/+server.rs`).

#[derive(Json, FromJson, Clone)]
pub struct Note {
    pub title: String,
}

#[derive(FromJson)]
pub struct New {
    pub title: String,
}

static NOTES: Table<Note> = Table::new();

pub fn all() -> Vec<Note> {
    NOTES.all().into_iter().map(|r| r.value).collect()
}

pub fn add(new: New) -> Note {
    let note = Note { title: new.title };
    NOTES.add(note.clone());
    note
}

pub fn find(id: u64) -> Option<Note> {
    NOTES.get(id).map(|r| r.value)
}

pub fn remove(id: u64) -> Option<()> {
    NOTES.remove(id).map(|_| ())
}
