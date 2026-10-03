/// Built once in `init`, read by every request.
pub struct Greeting(pub &'static str);

/// Who is signed in, found once per request by `before`.
pub struct User(pub String);

fn init() {
    wisp::provide(Greeting("hello from init"));
    wisp::csp("img-src 'self' https://img.example");
    wisp::users(&people::PEOPLE);
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

/// How many 5xx `report` has seen.
static REPORTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn report(cx: &mut Cx, err: &Error) {
    let _ = (cx, err);
    REPORTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Every reply says how many 5xx were reported before it.
fn after(_cx: &mut Cx, reply: &mut Reply) {
    let n = REPORTS.load(std::sync::atomic::Ordering::Relaxed);
    reply.headers.push(("x-reports".into(), format!("{n} so far").into()));
}
