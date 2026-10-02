---
// @feature auth
#[action]
fn default(email: Email, #[validate(min_len = 8)] password: String) {
    if db::USERS.find(|u| u.email == email).is_some() {
        return invalid("email", "Already signed up");
    }
    cx.sign_in(db::USERS.add(db::User { email, hash: wisp::password::hash(&password).await, avatar: None }));
    redirect("/dashboard")
}
---
<title>Sign up</title>
<form method="post">
  <input name="email" type="email">
  <input name="password" type="password">
  <button>Sign up</button>
</form>
