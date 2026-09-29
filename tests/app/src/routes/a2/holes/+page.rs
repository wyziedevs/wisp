struct Data {
    greeting: String,
}

fn load() -> Data {
    Data { greeting: "Hello <server>".into() }
}
