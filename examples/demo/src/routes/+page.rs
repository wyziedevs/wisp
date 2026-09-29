struct Data {
    count: i64,
}

fn load(cx: &mut Cx) -> Data {
    Data { count: count(cx) }
}

// The buttons post to these. Without JavaScript that is a normal form post;
// with it, wisp.js sends it in the background and updates the page in place.

#[action]
fn increment(cx: &mut Cx) {
    cx.set_cookie("count", count(cx) + 1);
}

#[action]
fn decrement(cx: &mut Cx) {
    cx.set_cookie("count", count(cx) - 1);
}

/// The count lives in a cookie, so every visitor has their own and it
/// survives a reload.
fn count(cx: &Cx) -> i64 {
    cx.cookie_or("count", 0)
}
