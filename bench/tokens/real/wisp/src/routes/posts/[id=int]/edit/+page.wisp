---
// @feature crud
#[action]
fn default(id: u64, post: Post) {
    POSTS.update(id, |p| *p = post).or_404()?;
    redirect("/posts")
}

let post = POSTS.get(id).or_404()?;
---
<title>Edit {post.title}</title>
<form method="post">
  <input name="title" value={post.title}>
  <textarea name="body">{post.body}</textarea>
  <button>Save</button>
</form>
