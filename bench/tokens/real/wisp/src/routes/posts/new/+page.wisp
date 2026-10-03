---
// @feature crud
#[action]
fn default(post: Post) {
    POSTS.add(post);
    // @feature live
    wisp::channel("posts").send("new");
    // @feature crud
    redirect("/posts")
}
---
<title>New post</title>
<form method="post">
  <input name="title">
  <textarea name="body"></textarea>
  <button>Create</button>
</form>
