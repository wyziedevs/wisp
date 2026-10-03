---
static POSTS: Table<String> = Table::new();

#[action]
fn add(text: String) {
    POSTS.add(text);
}

let posts = POSTS.page(cx, 2);
---
{#each posts as post}
  <p>{post}</p>
{/each}
{@pager posts}
