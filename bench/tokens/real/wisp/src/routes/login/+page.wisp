---
// @feature auth
#[action]
fn default(email: Email, password: String) {
    cx.login(&USERS, &email, &password).await?;
    redirect("/dashboard")
}
---
<title>Log in</title>
<form method="post" fields><button>Log in</button></form>
