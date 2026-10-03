// @feature data
#[model]
pub struct User {
    email: Email,
    hash: String,
    avatar: Option<Image>,
}

#[model]
pub struct Post {
    #[validate(len = 1..=100)]
    title: String,
    body: String,
}

pub static USERS: Table<User> = Table::saved();
pub static POSTS: Table<Post> = Table::saved();
