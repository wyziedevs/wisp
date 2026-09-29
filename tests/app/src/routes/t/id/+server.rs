/// The request's id, for the tests of `x-request-id`.
fn get(cx: &mut Cx) -> Response {
    Response::text(cx.request_id())
}
