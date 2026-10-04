---
// @feature form
fn default(#[validate(len = 1..=50)] name: String, email: Email) {
    eprintln!("{name} <{email}>");
    redirect("/")
}
---
<title>Contact</title>
<form fields><button>Send</button></form>
