---
// @feature auth
fn default(email: Email, password: Password) {
    cx.signup(User {
        email,
        password,
        avatar: None,
    })
    .await?;
    redirect("/dashboard")
}
---

<title>Sign up</title>
<form fields="Sign up" />
