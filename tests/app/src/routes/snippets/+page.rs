pub struct Data {
    pub items: Vec<(&'static str, u32)>,
}

pub fn load() -> Data {
    Data { items: vec![("pen", 2), ("ink", 5)] }
}
