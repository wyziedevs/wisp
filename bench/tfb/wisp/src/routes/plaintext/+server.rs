fn get() -> Response {
    Response::text("Hello, World!").with_header("content-type", "text/plain")
}
