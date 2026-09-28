use wisp::prelude::*;

pub struct Data {
    pub count: i64,
}

/// The count lives in a cookie, so every visitor has their own and it
/// survives a reload.
pub fn load(cx: &mut Cx) -> Data {
    Data { count: cx.cookie_or("count", 0) }
}

// The buttons post to these. Without JavaScript that is a normal form post;
// with it, wisp.js sends it in the background and updates the page in place.

#[action]
pub fn increment(cx: &mut Cx) {
    let count: i64 = cx.cookie_or("count", 0);
    cx.set_cookie("count", count + 1);
}

#[action]
pub fn decrement(cx: &mut Cx) {
    let count: i64 = cx.cookie_or("count", 0);
    cx.set_cookie("count", count - 1);
}
