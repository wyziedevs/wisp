use wisp::prelude::*;

pub struct Data {
    pub slug: String,
}

pub fn load(cx: &mut Cx) -> Data {
    Data { slug: cx.param("slug").to_string() }
}
