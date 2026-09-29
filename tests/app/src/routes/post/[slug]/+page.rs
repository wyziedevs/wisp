use wisp::prelude::*;

pub struct Data {
    pub slug: String,
}

pub fn load(cx: &mut Cx) -> Data {
    Data { slug: cx.param("slug").to_string() }
}

/// The pages `wisp build --static` writes for this route.
pub fn entries() -> Vec<&'static str> {
    vec!["hello", "second-post"]
}
