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
  <input aria-label="title" name="title">
  <textarea aria-label="body" name="body"></textarea>
  <button>Create</button>
</form>
