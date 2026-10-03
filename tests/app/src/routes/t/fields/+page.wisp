---
// `fields`: the inputs come from the action's parameters; a lone `fn default` is the action.
fn default(email: Email, #[validate(min_len = 8)] password: String, note: Option<String>, avatar: Option<Image>) {
    eprintln!("{email} {} {note:?} {}", password.len(), avatar.is_some());
    redirect("/t/fields")
}
---
<form method="post" fields><button>Go</button></form>
