// @feature live
fn get() -> Response {
    wisp::channel("posts").events()
}
