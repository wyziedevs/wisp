---
// @feature upload
#[action]
fn default(avatar: Image) {
    USERS.update(cx.signed_in()?, |u| u.avatar = Some(avatar));
    redirect("/dashboard")
}

cx.signed_in()?;
---
<title>Avatar</title>
<form method="post" fields><button>Upload</button></form>
