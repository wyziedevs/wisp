---
use people::{PEOPLE, Person};

#[action]
fn join(#[validate(len = 1..=40)] name: String, #[validate(min_len = 8)] password: String) {
    let id = PEOPLE.add(Person { name, hash: wisp::password::hash(&password).await?, avatar: None });
    cx.sign_in(id);
    redirect("/me")
}

#[action]
async fn enter(name: String, password: String) {
    let person = PEOPLE.find(|p| p.name == name);
    let hash = person.as_ref().map(|p| p.hash.as_str());
    if !wisp::password::check(&password, hash).await? {
        return invalid("password", "Wrong name or password");
    }
    cx.sign_in(person.unwrap().id);
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
