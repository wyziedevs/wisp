---
fn default(
    #[validate(len = 1..=100)] name: String,
    email: Email,
    #[validate(one_of = "hello help")] topic: String,
) {
    cx.flash(&format!("Thanks, {name}! We will write to {email} about {topic}."));
    redirect("/t/contact")
}
---

<title description="Write to us" image="/og.png">Contact</title>

{@flash}
<form fields />
