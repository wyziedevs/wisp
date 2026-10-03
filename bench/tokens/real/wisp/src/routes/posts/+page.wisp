---
// @feature crud
#[action]
fn remove(id: u64) {
    POSTS.remove(id);
}

let posts = POSTS.page(cx, 10);
---
<title>Posts</title>
<a href="/posts/new">New post</a>
{#each posts as post}
  <!-- @feature component -->
  <Details title={&post.title}>
    <!-- @feature crud -->
    <p>{post.body}</p>
    <a href="/posts/{post.id}/edit">Edit</a>
    <button action="?/remove&id={post.id}">Delete</button>
  </Details>
{/each}
{@pager posts}
