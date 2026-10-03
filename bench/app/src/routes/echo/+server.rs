// A validated JSON body: 200 with the same fields, or 422 listing the
// failing ones.

#[derive(Json, FromJson)]
struct Person {
    #[validate(len = 1..=50)]
    name: String,
    #[validate(email)]
    email: String,
    #[validate(min = 0, max = 150)]
    age: i64,
    #[validate(max_len = 10)]
    tags: Vec<String>,
}

fn post(body: Person) -> Person {
    body
}
