---
// @feature auth
fn default(email: Email, #[validate(min_len = 8)] password: Password) {
    cx.signup(User { email, password, avatar: None }).await?;
    redirect("/dashboard")
}
---
<title>Sign up</title>
<form fields><button>Sign up</button></form>
