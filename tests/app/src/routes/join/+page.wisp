---
use people::{PEOPLE, Person};

#[action]
fn join(#[validate(len = 1..=40)] name: String, #[validate(min_len = 8)] password: String) {
    let id = PEOPLE.add(Person { name, hash: wisp::password::hash(&password).await, avatar: None });
    cx.sign_in(id);
    redirect("/me")
}

#[action]
async fn enter(name: String, password: String) {
    let Some(person) = PEOPLE.find(|p| p.name == name) else {
        return invalid("name", "No one by that name");
    };
    if !wisp::password::verify(&password, &person.hash).await {
        return invalid("password", "Wrong password");
    }
    cx.sign_in(person.id);
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
