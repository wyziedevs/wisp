//! What AGENTS.md's snippets call: `src/db.rs`'s `pub` items are in every route file.

// src/db.rs
#[model] // Json + FromJson + Clone, all pub
struct Todo {
    #[validate(len = 1..=100)]
    text: String,
}
pub static TODOS: Table<Todo> = Table::saved(); // "todos"; Table::new() = memory

// src/db.rs: a `Password` field and `email` (or `name`) make a #[model] an Account
#[model]
struct User {
    #[unique]
    email: Email,
    password: Password,
}
pub static USERS: Table<User> = Table::saved();

#[model]
struct Post {
    title: String,
}

#[model]
struct Note {
    title: String,
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
    let post = Note { title: new.title };
    NOTES.add(post.clone());
    post
}

pub fn find(id: u64) -> Option<Note> {
    NOTES.get(id).map(|r| r.value)
}
