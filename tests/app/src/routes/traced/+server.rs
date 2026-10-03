/// A span of the app's own inside the request's, and the `traceparent`
/// that a call to another service would carry from inside it.
fn get() -> Response {
    let _work = wisp::span("work");
    Response::text(wisp::traceparent().unwrap_or_default())
}
