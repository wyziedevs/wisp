//! Members: who joined (`/join`), their password hashes and pictures
//! (`/me`, `/avatars/[id]`).

#[derive(Json, FromJson, Clone)]
pub struct Person {
    pub name: String,
    pub hash: String,
    pub avatar: Option<Image>,
}

pub static PEOPLE: Table<Person> = Table::saved("people");
