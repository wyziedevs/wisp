---
// @feature auth
#[action]
fn default(email: Email, password: String) {
    let user = db::USERS.find(|u| u.email == email);
    if !wisp::password::check(&password, user.as_ref().map(|u| u.hash.as_str())).await? {
        return invalid("email", "Wrong email or password");
    }
    cx.sign_in(user.unwrap().id);
    redirect("/dashboard")
}
---
<title>Log in</title>
<form method="post">
  <input name="email" type="email">
  <input name="password" type="password">
  <button>Log in</button>
</form>
