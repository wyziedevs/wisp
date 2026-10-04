---
// @feature crud
fn default(post: Post) {
    POSTS.add(post);
    redirect("/posts")
}
---
<title>New post</title>
<form fields><button>Create</button></form>
