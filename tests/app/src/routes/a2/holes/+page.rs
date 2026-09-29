pub struct Data {
    pub greeting: String,
}

pub fn load() -> Data {
    Data {
        greeting: "Hello <server>".into(),
    }
}
