// @feature data
#[model]
struct User {
    email: Email,
    password: Password,
    avatar: Option<Image>,
}

#[model]
struct Post {
    #[validate(len = 1..=100)]
    title: String,
    body: String,
}

pub static USERS: Table<User> = Table::saved();
pub static POSTS: Table<Post> = Table::saved()
// @feature live
    .live();
