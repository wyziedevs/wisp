fn get(id: String, q: String, cx: &Cx) -> Response {
    Response::text(format!("id={id} q={q} sid={}", cx.cookie_or("sid", "none".to_string())))
}
