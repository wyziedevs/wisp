use wisp::prelude::*;

#[derive(Json)]
pub struct Data {
    pub server: String,
}

pub fn load() -> Data {
    Data {
        server: "server".into(),
    }
}
