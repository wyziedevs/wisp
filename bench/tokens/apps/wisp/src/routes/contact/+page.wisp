---
// @feature form
#[action]
fn default(#[validate(len = 1..=50)] name: String, #[validate(email)] email: String) -> Result {
    eprintln!("{name} <{email}>");
    redirect("/")
}
---
<title>Contact</title>
<form method="post">
  <input name="name">
  <input name="email">
  <button>Send</button>
</form>
