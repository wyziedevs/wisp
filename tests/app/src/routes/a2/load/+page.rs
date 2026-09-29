#[derive(Json)]
struct Data {
    server: String,
}

fn load() -> Data {
    Data { server: "server".into() }
}
