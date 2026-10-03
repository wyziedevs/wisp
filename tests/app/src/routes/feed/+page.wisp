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
{#if let Some(href) = posts.prev}<a {href}>Newer</a>{/if}
{#if let Some(href) = posts.next}<a {href}>Older</a>{/if}
