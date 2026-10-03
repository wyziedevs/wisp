---
#[action]                                       // sign up
fn signup(email: Email, #[validate(min_len = 8)] password: String) {
    cx.signup(&USERS, User { email, hash: password }).await?;  // hashes it; 422 if taken
    redirect("/me")
}
#[action]                                       // log in
fn login(email: Email, password: String) {
    cx.login(&USERS, &email, &password).await?; // 422 for either wrong, equally slow
    redirect("/me")
}
let me = cx.user(&USERS)?;                      // Row<User>, or 303 to /login
---
<h1>{me.email}</h1>
