---
use people::{PEOPLE, Person};

#[action]
fn join(#[validate(len = 1..=40)] name: String, #[validate(min_len = 8)] password: String) {
    cx.signup(&PEOPLE, Person { name, hash: password, avatar: None }).await?;
    redirect("/me")
}

#[action]
fn enter(name: String, password: String) {
    cx.login(&PEOPLE, &name, &password).await?;
    redirect("/me")
}
---
<form action="?/join">
  <input name="name">
  <input type="password" name="password">
</form>
<form action="?/enter">
  <input name="name">
  <input type="password" name="password">
</form>
<p class="problem">{cx.problem("password")}</p>
