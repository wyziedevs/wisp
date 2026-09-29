use crate::hooks::User;

struct Data {
    name: String,
    /// What signing in left for this page, the first time it shows.
    hello: Option<String>,
}

/// `before` sends visitors who are not signed in to /login, so a `User`
/// is always there.
fn load(cx: &mut Cx) -> Result<Data> {
    let name = cx.get::<User>().or_status(401)?.0.clone();
    Ok(Data { name, hello: cx.flashed() })
}
