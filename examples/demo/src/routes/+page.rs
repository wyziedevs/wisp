use wisp::prelude::*;

pub struct Data {
    pub count: i64,
}

pub async fn load(cx: &mut Cx) -> Result<Data> {
    Ok(Data { count: count(cx) })
}

// The buttons post to these. Without JavaScript that is a normal form post;
// with it, wisp.js sends it in the background and updates the page in place.

#[action]
pub async fn increment(cx: &mut Cx) -> Result<()> {
    add(cx, 1);
    Ok(())
}

#[action]
pub async fn decrement(cx: &mut Cx) -> Result<()> {
    add(cx, -1);
    Ok(())
}

/// The count lives in a cookie, so every visitor has their own and it
/// survives a reload.
fn count(cx: &Cx) -> i64 {
    cx.cookie("count").and_then(|c| c.parse().ok()).unwrap_or(0)
}

fn add(cx: &mut Cx, n: i64) {
    let count = count(cx).saturating_add(n);
    cx.set_cookie("count", &count.to_string());
}
