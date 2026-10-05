---
// @feature upload
fn default(avatar: Image) {
    USERS.update(cx.signed_in()?, |u| u.avatar = Some(avatar));
    redirect("/dashboard")
}

cx.signed_in()?;
---

<title>Avatar</title>
<form fields="Upload" />
