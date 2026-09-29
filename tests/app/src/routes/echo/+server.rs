const BODY_LIMIT: usize = 64 * wisp::KB;

/// The body's length and text, to check how it arrived.
fn post(cx: &mut Cx) -> Response {
    Response::text(format!("{}:{}", cx.body().len(), String::from_utf8_lossy(cx.body())))
}
