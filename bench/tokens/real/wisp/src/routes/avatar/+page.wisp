---
// @feature upload
#[action]
fn default(#[validate(max_size = 1 * MB)] avatar: Image) {
    let user = cx.user(&db::USERS)?;
    db::USERS.update(user.id, |u| u.avatar = Some(avatar));
    redirect("/dashboard")
}

cx.signed_in()?;
---
<title>Avatar</title>
<form method="post" enctype="multipart/form-data">
  <input name="avatar" type="file" accept="image/*">
  <button>Upload</button>
</form>
