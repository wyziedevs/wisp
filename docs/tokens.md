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
commands are counted, since the agent must write them, and the lines it
adds or changes in generated files. Rails' api is `rails g scaffold` in an
`--api` app, which writes the controller; the same api written by hand is
the last row.

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
| **Wisp** | **44** (1) | **191** (1) | **59** (1) | **292** (4) | **586** |
| Wisp, previous version | 44 (1) | 307 (1) | 606 (4) | 302 (4) | 1259 |
| Wisp, before blocks | 44 (1) | 385 (2) | 670 (5) | 381 (6) | 1480 |
| SvelteKit | 49 (1) | 373 (2) | 622 (4) | 389 (5) | 1433 |
| Next.js | 77 (1) | 501 (4) | 722 (4) | 461 (4) | 1761 |
| Nuxt | 49 (1) | 473 (5) | 572 (7) | 332 (5) | 1426 |
| Axum | 248 (1) | 641 (1) | 1056 (1) | 462 (1) | 2407 |
| FastAPI | 123 (1) | 408 (2) | 494 (1) | 446 (4) | 1471 |
| Rails | 132 (3) | 340 (5) | 132 (4) | 357 (7) | 961 |
| Rails, api by hand | 132 (3) | 340 (5) | 357 (4) | 357 (7) | 1186 |

Wisp is the shortest on every app, and in all 39% shorter than the next
(Rails with its scaffold) and 54% shorter than it was. The api is a type:
Rails' scaffold is the only other that comes close, with a generator
command, a model, a route and a controller to edit. Characters / 4 ranks
them the same way (Wisp 346, Rails 726, SvelteKit 959).

The apps are not equal in what they do, and the difference favors Wisp's
api. Its 59 tokens keep the notes across restarts and crashes (a log file
per table in `WISP_DATA`); of the others only Rails does (SQLite, through
Active Record), and the rest keep them in memory, as the task allows. The
same 59 tokens also answer filters by field, sorting, cursor pages, field
selection, ETags with 304 and 412, bulk creates and idempotent retries,
which no other version here has. Asked of the others, those would cost
them more tokens; in Wisp they cost none. What a real API adds in Wisp is
counted in the usual way: a `created_at: String` field is 6 tokens, a hook
such as `fn before_create(note: &mut Note) -> Result { Ok(()) }` 22 with
its body, and keeping every table in SQLite instead of log files is a
`wisp::Store` of 373 (docs/api.md), written once per app.

## What changed to get here

Where Wisp cost more, the framework changed, not the apps. This version:

| Was | Now | Saves |
|---|---|---|
| a store, a model, two `+server.rs` with five handlers, an auth hook | `#[derive(Rest)]` on the struct, `#[rest(write = "API_KEY")]`, saved across restarts | 547 of the api's 606 |
| `[id=int]/+server.rs` beside `+server.rs` | a handler that takes `id` serves `/[id]`; `list` is the folder's GET | a file, its path, its imports |
| `if text.trim().is_empty() \|\| text.len() > 100 { return invalid(..) }` | `#[validate(len = 1..=100)] text: String` on the action | the check and its message |
| `<form method="post" action="?/add">` | `<form action="?/add">` | 5 per form |
| `value={cx.input("text")}` | nothing: an action form's inputs keep what was sent | 12 per input |
| `{#if let Some(e) = cx.problem("text")}<p>{e}</p>{/if}` | `{cx.problem("text")}`: an `Option` shows nothing for `None` | 21 |
| `<form …><button name="id" value={t.id}>x</button></form>` | `<button action="?/remove&id={t.id}">x</button>` | 11 |
| `Shared<Vec<String>>`, indexes | `Table<String>`: ids, `add`, `all`, `remove` | 8, and stable ids |
| `<head><title>…</title></head>` | `<title>…</title>` | 6 |
| `{@render children()}` | `<slot />` | 4 |

Before that:

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

Every old form still works, but for one: a `+server.rs` handler that took
an `id` from the query, in a folder with no `[id]`, now answers at `/[id]`.

## The Wisp versions

```rust
// api: src/routes/api/notes/+server.rs
#[derive(Rest)]
#[rest(write = "API_KEY")]
struct Note {
    #[validate(len = 1..=200)]
    title: String,
    done: bool,
}
```

```html
<!-- todo: src/routes/+page.wisp -->
---
static TODOS: Table<String> = Table::new();

#[action]
fn add(#[validate(len = 1..=100)] text: String) {
    TODOS.add(text);
}

#[action]
fn remove(id: u64) {
    TODOS.remove(id);
}
---
<form action="?/add">
  <input name="text">
  <button>Add</button>
  {cx.problem("text")}
</form>
<ul>
  {#each TODOS.all() as todo}
    <li>{todo} <button action="?/remove&id={todo.id}">x</button></li>
  {/each}
</ul>
```

```html
<!-- blog: src/routes/blog/[slug]/+page.wisp -->
---
let post = posts::POSTS.iter().find(|p| p.slug == slug).or_404()?;
---
<title>{post.title}</title>
<h1>{post.title}</h1>
<p>{post.body}</p>
```

The competitors' sources, the counting script (`count.py`) and the script
that compiles the Wisp versions (`verify.py`) are kept out of the
repository, so its size stays Wisp's own.
