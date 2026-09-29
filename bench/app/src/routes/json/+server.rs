// TechEmpower's "json": one small object, serialized per request.

#[derive(Json)]
struct Message {
    message: &'static str,
}

fn get() -> Message {
    Message { message: "Hello, World!" }
}
