---
// @feature auth
#[action]
fn default(email: Email, #[validate(min_len = 8)] password: Password) {
    cx.signup(&USERS, User { email, password, avatar: None }).await?;
    redirect("/dashboard")
}
---
<title>Sign up</title>
<form method="post" fields><button>Sign up</button></form>
