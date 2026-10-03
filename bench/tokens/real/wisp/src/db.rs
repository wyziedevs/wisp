// @feature data
#[derive(Json, FromJson, Clone)]
pub struct User {
    pub email: Email,
    pub hash: String,
    pub avatar: Option<Image>,
}

#[derive(Json, FromJson, Clone)]
pub struct Post {
    #[validate(len = 1..=100)]
    pub title: String,
    pub body: String,
}

pub static USERS: Table<User> = Table::saved("users");
pub static POSTS: Table<Post> = Table::saved("posts");
