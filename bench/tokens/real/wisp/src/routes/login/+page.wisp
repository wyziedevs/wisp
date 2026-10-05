---
// @feature auth
fn default(email: Email, password: String) {
    cx.login(&email, &password).await?;
    redirect("/dashboard")
}
---

<title>Log in</title>
<form fields="Log in" />
