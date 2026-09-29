struct Data {
    items: Vec<(&'static str, u32)>,
}

fn load() -> Data {
    Data { items: vec![("pen", 2), ("ink", 5)] }
}
