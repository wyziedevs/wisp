---
// @feature crud
#[action]
fn remove(id: u64) {
    db::POSTS.remove(id);
}

let posts = db::POSTS.page(cx, 10);
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
{#if let Some(href) = &posts.prev}<a {href}>Newer</a>{/if}
{#if let Some(href) = &posts.next}<a {href}>Older</a>{/if}
<!-- @feature live -->
<script>listen('/posts/events', invalidate)</script>
