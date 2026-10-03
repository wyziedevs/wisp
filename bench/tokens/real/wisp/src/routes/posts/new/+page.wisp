---
// @feature crud
#[action]
fn default(post: Post) {
    POSTS.add(post);
    redirect("/posts")
}
---
<title>New post</title>
<form method="post">
  <input name="title">
  <textarea name="body"></textarea>
  <button>Create</button>
</form>
