---
// @feature crud
#[action]
fn default(id: u64, post: db::Post) {
    db::POSTS.update(id, |p| *p = post).or_404()?;
    redirect("/posts")
}

let post = db::POSTS.get(id).or_404()?;
---
<title>Edit {post.title}</title>
<form method="post">
  <input name="title" value={post.title}>
  <textarea name="body">{post.body}</textarea>
  <button>Save</button>
</form>
