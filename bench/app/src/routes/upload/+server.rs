// A body of any type up to 8 MiB, answered with its size.

const BODY_LIMIT: usize = 8 * wisp::MB;

fn post(cx: &Cx) -> Response {
    Response::text(cx.body().len().to_string())
}
