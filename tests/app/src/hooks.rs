/// Built once in `init`, read by every request.
pub struct Greeting(pub &'static str);

/// Who is signed in, found once per request by `before`.
pub struct User(pub String);

fn init() {
    wisp::provide(Greeting("hello from init"));
}

fn before(cx: &mut Cx) -> Result<Option<Response>> {
    cx.set_header("x-app", "test");
    if cx.method == Method::Options {
        return Ok(Some(Response::empty(204).with_header("access-control-allow-origin", "*")));
    }
    if let Some(name) = cx.signed_cookie("user") {
        let user = User(name.to_string());
        cx.set(user);
    }
    if cx.path().starts_with("/admin") && cx.get::<User>().is_none() {
        return redirect("/login");
    }
    Ok(None)
}
