//! Members: who joined (`/join`), their password hashes and pictures
//! (`/me`, `/avatars/[id]`).

#[model]
pub struct Person {
    name: String,
    hash: String,
    avatar: Option<Image>,
}

pub static PEOPLE: Table<Person> = Table::saved();
