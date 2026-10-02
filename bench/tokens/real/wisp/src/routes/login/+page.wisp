---
// @feature auth
#[action]
fn default(email: Email, password: String) {
    match db::USERS.find(|u| u.email == email) {
        Some(u) if wisp::password::verify(&password, &u.hash).await => {
            cx.sign_in(u.id);
            redirect("/dashboard")
        }
        _ => invalid("email", "Wrong email or password"),
    }
}
---
<title>Log in</title>
<form method="post">
  <input name="email" type="email">
  <input name="password" type="password">
  <button>Log in</button>
</form>
