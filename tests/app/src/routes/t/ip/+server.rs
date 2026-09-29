/// The TCP peer and the address `cx.client_ip()` settles on, for the tests of
/// `WISP_CLIENT_IP_HEADER`.
fn get(cx: &mut Cx) -> Response {
    Response::text(format!("{}|{}", cx.peer().ip(), cx.client_ip()))
}
