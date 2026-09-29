---
#[action]
fn default(name: String) -> Result {
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return error(400, "A name is letters and digits");
    }
    cx.set_signed_cookie("user", &name);
    cx.flash(&format!("Hello, {name}!"));
    redirect("/admin")
}

#[action]
fn logout() -> Result {
    cx.set_signed_cookie("user", "");
    redirect("/")
}
---
<form method="post">
  <input name="name">
  <button>Sign in</button>
</form>
