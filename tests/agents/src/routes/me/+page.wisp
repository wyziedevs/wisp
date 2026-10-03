---
#[derive(Json, FromJson, Clone)]
struct User {
    name: String,
    hash: String,
}

static USERS: Table<User> = Table::saved("users");   // User { name, hash: String, .. }

#[action]
fn join(name: String, #[validate(min_len = 8)] password: String) {
    let id = USERS.add(User { name, hash: wisp::password::hash(&password).await? });
    cx.sign_in(id);                     // signed cookie, 30 days, a new one
    redirect("/me")
}
#[action]
fn login(name: String, password: String) {
    let user = USERS.find(|u| u.name == name);
    let hash = user.as_ref().map(|u| u.hash.as_str());  // None: as slow, so names stay secret
    if !wisp::password::check(&password, hash).await? { return invalid("password", "Wrong name or password"); }
    cx.sign_in(user.unwrap().id);
    redirect("/me")
}
let me = cx.user(&USERS)?;              // a members' page: Row<User>, or 303 to /login
---
<h1>{me.name}</h1>
