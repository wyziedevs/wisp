# Tokens

Most app code is now written by AI, so what an app costs to write is
measured in tokens. The first of Wisp's principles ([design.md](design.md))
is to keep that number low. This page measures it: the same four small apps,
written idiomatically and as short as each framework allows, in Wisp and in
six others.

## The apps

- **counter**: a button that counts clicks.
- **todo**: a list kept in memory, a form that adds an item (1 to 100
  characters; otherwise a 422 that shows the problem and keeps what was
  typed), and a delete button per item.
- **api**: JSON CRUD for notes (`id`, `title`, `done`): list, get, create
  (title 1 to 200 characters, else 422), update, delete; 404 for a missing
  note; writes need `Authorization: Bearer $API_KEY`.
- **blog**: a layout with a header, an index of posts, and `/blog/[slug]`
  with the post's title in `<title>`, 404 for a missing one.

## Method

Counted: every file a developer (or an agent) writes by hand, beyond what
the framework's generator gives, plus each file's path (writing a file means
naming it, so two files cost more than one). Manifests (`Cargo.toml`,
`package.json`, `Gemfile`) are left out everywhere; for Rails the generator
commands are counted, since the agent must write them.

There is no tokenizer offline, so the count is an estimate of a BPE code
tokenizer (cl100k-like): a newline with its indentation is 1 token; spaces
join the word after them; identifiers split at `_` and camelCase humps, each
part 1 token up to 8 letters and 1 per 6 after that; digits 1 per 3; common
operators (`::` `->` `=>` `==` `</` `/>` `{{` `<%=` ...) 1; any other
punctuation character 1. Characters / 4, the usual rule of thumb, ranks the
frameworks the same way.

The competitor versions: SvelteKit 2 with Svelte 5 runes and `use:enhance`;
Next.js 15 app router with server actions and `useActionState`; Nuxt 3 with
server routes and `useFetch`; Axum 0.8 with maud and serde; FastAPI with
Jinja2 and pydantic; Rails 8 with Active Record. The Wisp versions are built
by a script, so every counted line compiles.

## Results

Estimated tokens (files):

| Framework | counter | todo | api | blog | total |
|---|---:|---:|---:|---:|---:|
| **Wisp** | **44** (1) | **307** (1) | 606 (4) | **302** (4) | 1259 |
| Wisp, before blocks | 44 (1) | 385 (2) | 670 (5) | 381 (6) | 1480 |
| SvelteKit | 49 (1) | 373 (2) | 622 (4) | 389 (5) | 1433 |
| Next.js | 77 (1) | 501 (4) | 722 (4) | 461 (4) | 1761 |
| Nuxt | 49 (1) | 473 (5) | 572 (7) | 332 (5) | 1426 |
| Axum | 248 (1) | 641 (1) | 1056 (1) | 462 (1) | 2407 |
| FastAPI | 123 (1) | 408 (2) | 494 (1) | 446 (4) | 1471 |
| Rails | 132 (3) | 340 (5) | **357** (4) | 357 (7) | **1186** |

Wisp is the shortest for pages (counter, todo, blog) and 15% shorter than it
was. It trails on the JSON API: Rails' Active Record writes the storage,
validation and JSON for it, and FastAPI's pydantic models merge a partial
update in one line; Wisp spells both out.

## What changed to get here

Where Wisp cost more, the framework changed, not the apps:

| Was | Now | Saves |
|---|---|---|
| `+page.rs` beside `+page.wisp` | one `+page.wisp`, Rust in a `---` block | a file and its path |
| `struct Data { … }`, `fn load(cx: &mut Cx) -> Data { Data { … } }` | the block's statements are the load; the markup reads their names | every field name twice, and its type |
| `fn load(slug: String) -> Data { Data { slug } }` | route parameters are locals, with no Rust at all | the whole file |
| `#[action] fn add(cx: &mut Cx, …)` | `#[action] fn add(…)`, `cx` added when the body uses it | `cx: &mut Cx` per action |
| `cx.fail(422, P(..))` + `cx.take()` in `load` + a `Data` field | `return invalid("f", "…")`; `cx.problem("f")`, `cx.input("f")` in markup | the plumbing |
| `-> Result<()>` | `-> Result` | 3 per function |
| `Mutex` import, `.lock().unwrap()` | `Shared<T>` in the prelude, `.lock()` | the import and each `unwrap` |
| `mod notes;` in `main.rs`, `use crate::notes::…` | `src/notes.rs` is a module; routes say `notes::` | a file edit and `crate::` |
| `<wisp:head>` | `<head>` | 4 per page |

Every old form still works.

## The Wisp versions

```html
<!-- todo: src/routes/+page.wisp -->
---
static TODOS: Shared<Vec<String>> = Shared::new(Vec::new());

#[action]
fn add(text: String) -> Result {
    if text.trim().is_empty() || text.len() > 100 {
        return invalid("text", "Write 1 to 100 characters");
    }
    TODOS.lock().push(text);
    Ok(())
}

#[action]
fn remove(i: usize) {
    TODOS.lock().remove(i);
}
---
<form method="post" action="?/add">
  <input name="text" value={cx.input("text")}>
  <button>Add</button>
  {#if let Some(e) = cx.problem("text")}<p>{e}</p>{/if}
</form>
<ul>
  {#each TODOS.lock().iter() as todo, i}
    <li>{todo}
      <form method="post" action="?/remove"><button name="i" value={i}>x</button></form>
    </li>
  {/each}
</ul>
```

```html
<!-- blog: src/routes/blog/[slug]/+page.wisp -->
---
let post = posts::POSTS.iter().find(|p| p.slug == slug).or_404()?;
---
<head><title>{post.title}</title></head>
<h1>{post.title}</h1>
<p>{post.body}</p>
```

The competitors' sources, the counting script (`count.py`) and the script
that compiles the Wisp versions (`verify.py`) are kept out of the
repository, so its size stays Wisp's own.
