// 1000 rows built and serialized per request.

#[derive(Json)]
struct User {
    id: u32,
    name: String,
    email: String,
    active: bool,
}

fn get() -> Vec<User> {
    (0..1000)
        .map(|i| User {
            id: i,
            name: format!("user {i}"),
            email: format!("user{i}@example.com"),
            active: i % 3 != 0,
        })
        .collect()
}
