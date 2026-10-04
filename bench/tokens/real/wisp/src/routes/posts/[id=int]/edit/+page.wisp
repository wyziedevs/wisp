---
// @feature crud
fn default(id: u64, post: Post) {
    POSTS.set(id, post).or_404()?;
    redirect("/posts")
}

let post = POSTS.get(id).or_404()?;
---
<title>Edit {post.title}</title>
<form fields={post}><button>Save</button></form>
