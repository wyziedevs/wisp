---
fn default(#[validate(len = 1..=100)] name: String, email: Email) {
    cx.flash(&format!("Thanks, {name}!"));
    redirect("/t/contact")
}
---

<title description="Write to us" image="/og.png">Contact</title>

{@flash}
<form fields><button>Send</button></form>
