use wisp::prelude::*;

pub struct Data {
    pub n: u64,
}

/// The matcher let only digits through.
pub fn load(cx: &mut Cx) -> Data {
    Data { n: cx.param("n").parse().unwrap() }
}
