#[derive(Json)]
struct Message {
    message: &'static str,
}

fn get() -> Message {
    Message { message: "Hello, World!" }
}
