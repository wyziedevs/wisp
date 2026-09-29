use wisp::prelude::*;

pub struct Data {
    pub w: String,
}

pub fn load(cx: &mut Cx) -> Data {
    Data { w: cx.param("w").to_string() }
}
