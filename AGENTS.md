# Wisp: reference for coding agents

Wisp is a fast, fun web framework for Rust. File routes, `.wisp` templates
compiled to Rust, form actions, optional browser reactivity, one binary.

**Design rule (non-negotiable), in order:** 1. ultra fast, 2. cheap (fewest
tokens to write app code: AI writes most code), 3. durable (every fast path
proven at startup with a fallback; nothing after startup can take the process
down), 4. flexible. Developer happiness last. Wisp code:
Carmack style, minimal deps, no `unsafe` (but the Linux io_uring and epoll
drivers, `uring.rs` and `epoll.rs`, and the edge exports), no dead code,
zero warnings.

## Files

```
src/main.rs                 wisp::main!();   (generated; leave it)
src/app.html                shell with %wisp.head% %wisp.body% (optional)
src/app.css | app.scss      served at /_app/app.css (Tailwind if it imports it; Sass)
src/hooks.rs                fn init() once; fn before(cx) every request
src/db.rs                   models and tables; its `pub` items are in every route file
src/NAME.rs                 any module, no `mod` line: `NAME::f()` everywhere
src/components/Card.wisp    <Card title={x}>…</Card>
src/params/word.rs          fn matches(s: &str) -> bool, for [x=word]
src/routes/…/+page.wisp     page: optional `---` Rust block, then markup
src/routes/…/+layout.wisp   wraps pages below; must <slot /> (or {@render children()})
src/routes/…/+error.wisp    error page; has `status`, `message`, `cx`
src/routes/…/+server.rs     endpoints: fn get/post/put/patch/delete/list
static/…                    served at /
```

Folders: `blog` static, `[slug]` param, `[[lang]]` optional, `[...rest]`
rest, `[id=int]` digits (u64), `[x=word]` custom matcher, `(group)` not in
URL. `+page.rs` (`struct Data` + `fn load(..) -> Data`, which the markup reads
by name) and `+layout.rs` work instead of a block.

## A page

```rust
// src/db.rs
#[model]                       // Json + FromJson + Clone, fields pub
pub struct Todo {
    #[validate(len = 1..=100)]
    text: String,
}
pub static TODOS: Table<Todo> = Table::saved();   // "todos"; Table::new() = memory
```
```html
---
#[action]
fn add(todo: Todo) {
    TODOS.add(todo);
}

let count = TODOS.len();
---
<title>Todos ({count})</title>
<form action="?/add" fields><button>Add</button></form>
{#each TODOS.all() as todo}
  <p>{todo.text}</p>
{/each}
```

Block rules:
- Items (`fn struct enum use static const impl #[action]`) go in the page
  module. Statements are the page's load: they run per request (GET, and
  after an action) before render, in `async fn(cx: &mut Cx) -> Result`:
  `.await`, `?`, `return redirect("/x")`, `return error(404, "Gone")`; the
  markup reads their names. Layout blocks: sync, `cx: &Cx`.
- Route params are locals: `slug: String`, `[id=int]` → `id: u64`,
  `[[lang]]` → `Option<String>`. Even with no block.
- No `use` lines: prelude = `Cx Response Result Error Email Image Json
  FromJson Rest Cookie Method Value Shared Table Row RateLimit OrStatus KB MB
  action error invalid model redirect` and `src/db.rs`'s `pub` items (local
  names win). `Result` alone = `Result<()>`.
- `const CACHE: u32 = 60;` (page or `+server.rs`) keeps a GET's answer 60 s
  per worker (ETag, 304), but never for a request with a cookie or
  `authorization` (`CACHE_PUBLIC`: all), nor one that sets a cookie; not in dev.
- `.await` in markup outside any block runs with the statements:
  `{#each items().await as item}` (not in bodies, layouts, components). A
  page that reads nothing is baked at build.

## Actions (form posts)

```rust
#[action]
fn like(id: u64, email: Email, note: Option<String>, agree: bool, tags: Vec<String>) {
    cx.flash("Liked");                  // cx is added when the body uses it
    redirect("/")                       // 303; or end in `;` to re-render the page
}
```

- No `->`: returns `Result` (`?`, `return error(..)`, end in `redirect(..)`
  or `;`); `.await` makes it async. Or return `Response`/`Option<Response>`
  to answer instead of the page. A lone `fn default` needs no `#[action]`.
- `<form action="?/like">` posts (`method="post"` is added); `<form
  method="post">` → `fn default`; `<button action="?/rm&id={x.id}">` outside
  a form is a one-button form.
- `<form action="?/join" fields>` writes a labelled input per param: `Email`
  → `type=email`, `password`/`*_password` → password, `Image` → file, `bool`
  → checkbox, numbers → number, else text. Add your own button; for a
  textarea or select write the inputs yourself.
- Params by name: route param, then form, then query. `T` required (400
  missing; not a `T` → 422 by field), `Option<T>` missing/blank → None,
  `bool` checkbox, `Vec<T>` repeated, `&str`, `Email`, or a `#[model]`
  struct (the page's or `src/*.rs`'s): `fn default(post: Post)` reads its
  fields by name, blank = missing.
- Uploads: `avatar: Image` (`Option<Image>` may be empty): PNG, JPEG, GIF,
  WebP or AVIF by its bytes (not SVG), else 422; at most 2 MB
  (`wisp::MAX_SIZE`) unless `#[validate(max_size = 5 * MB)]`. The form gets
  `enctype="multipart/form-data"` and the file input `accept="image/*"`.
  Keep it in a table field, serve it with `fn get(id: u64) -> Option<Image> {
  USERS.get(id)?.value.avatar }` in `avatars/[id=int]/+server.rs`.
- Rules: `#[validate(len = 1..=100)]` (also `min max min_len max_len email`)
  or `return invalid("field", "msg")` → 422, the page re-rendered listing
  every failing field. Inputs get the matching browser checks (`required`,
  `minlength`, `type=email`, `min`/`max`); the server checks all; a button
  with `formaction="?/other"` skips them.
- Each named `<input>`, `<textarea>`, `<select>` of an action form shows what
  was sent again, else its own value (`value={post.title}`, `value="x"`,
  `<textarea name="body">{post.body}</textarea>`, `<select name="kind"
  value={post.kind}>`), then `<small class="problem">msg</small>` (passwords,
  files: the problem only); `{cx.problem("field")}` puts it elsewhere. Other
  errors → error page. Same-origin checked. Works without JS.

## Templates (Rust on the server)

| Syntax | Meaning |
|---|---|
| `{expr}` | escaped Display; an `Option` shows nothing for None |
| `{@html expr}` | raw (trusted only) |
| `attr={expr}` | quoted+escaped; `Option` → left out when None |
| `disabled={bool}` `<a {href}>` `class:on={bool}` | boolean attr, `href={href}`, toggled class |
| `{#if c}…{:else if c}…{:else}…{/if}` | `if let Some(x) = y` works |
| `{#each list as item, i}…{:else}…{/each}` | `{:else}` when empty |
| `{#match e}{:case P}…{/match}` | match |
| `{@const x = expr}` | let |
| `{#snippet row(a, b)}…{/snippet}` `{@render row(x, 1)}` | local markup fn |
| `{@pager posts}` | Newer/Older links of a `Table::page` |
| `<head>…</head>` | into the document head; a top-level `<title>` goes there alone |
| `<slot />` or `{@render children()}` | layout/component slot |
| `cx` | the request (`&Cx`) in pages, layouts, error pages |

Holes can't go in `on*` attrs, tag names, `javascript:` URLs, SVG animation
values or `<meta http-equiv>`; `<script>`/`<style>` bodies have none.

## Components (`src/components/Name.wisp`)

```html
{@props title, count: u32 = 0, featured: bool = false, row: Snippet<&Post, usize>}
<h2>{title}{#if featured} ★{/if}</h2>{@render children()}
```
Use: `<Card title={post.title} count={3} featured>kids</Card>`. Props are
checked at build (no type = `&str`). No `---` block in components.

## Browser code (JavaScript, same file)

`{…}` is Rust on the server; quoted directive values and `{:…}` are JS.

`<button on:click="count++">{:count}</button>` with `<script>let count = 0</script>`
(top-level lets are state). A page's Rust names are browser values by name (`items`, `data.items`).
`bind:value="q"` with no `let q` declares it, so a live search needs no
script: `<input bind:value="q">` `{:#each items.filter((i) => matches(i.name,
q)) as i}…{:/each}`. Directives `on:click` (`.prevent .once .debounce.300ms`…),
`bind:value|checked|this`, `:attr="js"`, `:text`, `class:x="js"`,
`transition:fade`, `use:action`; client blocks `{:#if}` `{:#each}`; runes
`$state $derived $effect $props`; helpers `onMount listen goto invalidate
matches`. Values sent to JS must be `#[model]` or `#[derive(Json)]`. Stores,
npm (`wisp add`), islands, the rest: docs/client.md.

## Endpoints (`+server.rs`)

A whole JSON API, saved across restarts (`src/routes/api/notes/+server.rs`):

```rust
#[derive(Rest)]                    // Json + FromJson + Note::table()
#[rest(write = "API_KEY")]         // writes need Bearer $API_KEY
struct Note {
    #[validate(len = 1..=200)]
    title: String,
    done: bool,                    // left out: false; Vec: []; Option: None
    created_at: String,            // set by Wisp (also updated_at)
}
fn before_create(note: &mut Note) -> Result { Ok(()) }  // also before_update, after_*
```
→ GET/POST `/api/notes`, GET/PUT/PATCH/DELETE `/api/notes/[id]`; rows are
`{"id":1,…}`; 201, 404, 422 by field. Filters, sorting, pages, ETags, ndjson,
idempotency, RFC 9457, webhooks, OpenAPI, TypeScript client: docs/api.md. A
handler the file writes replaces that one; by hand:

```rust
fn list() -> Vec<Note> { db::all() }                  // GET /api/notes
fn post(body: New) -> Response { Response::created(&db::add(body)) }
fn get(id: u64) -> Option<Note> { db::find(id) }      // `id` → /api/notes/[id]
```
`fn before(cx) -> Result` in the file runs before each handler.

`body: T` = the JSON body (422 with `errors` by field); other params by name
as for actions. Returns: a `#[model]`/`Json` value → 200; nothing → 204;
`Response`; `Option<T>` (None → 404). Endpoint and `/api` errors are JSON
`{"status","code","error","errors"}` (`Error::new(409, "x").with_code("taken")`).
`const BODY_LIMIT: usize = 20 * wisp::MB;`. Rows live in `WISP_DATA` log
files; `wisp::store(MyDb)` in `init` uses any DB.

## Members

```rust
// src/db.rs: `hash` and `email` (or `name`) make a #[model] an Account
#[model]
pub struct User { email: Email, hash: String }
pub static USERS: Table<User> = Table::saved();

#[action]                                       // sign up
fn default(email: Email, #[validate(min_len = 8)] password: String) {
    cx.signup(&USERS, User { email, hash: password }).await?;  // hashes it; 422 if taken
    redirect("/me")
}
#[action]                                       // log in
fn default(email: Email, password: String) {
    cx.login(&USERS, &email, &password).await?; // 422 for either wrong, equally slow
    redirect("/me")
}
let me = cx.user(&USERS)?;                      // Row<User>, or 303 to /login
```
Both sign in. `wisp::users(&db::USERS)` in `init` makes it `cx.user()`.
`cx.signed_in()?` is the id (`.ok()`: no redirect; JSON clients get 401),
`cx.sign_out()`, `wisp::sign_out_everywhere(id)?`, `wisp::sign_in_page("/enter")`.
`wisp::login`/`signup` are these without a Cx. Hashes: PBKDF2-SHA256, ~0.2 s
off the worker (`RateLimit` sign-ins); by hand `wisp::password::{hash, check}`.
`cx.need(&USERS, |u| u.admin)?` is the Row, 403 if not allowed. More:
`docs/auth.md` (`token_for`/`untoken_for` links, `totp`, `oauth`, `mail`,
`fetch`).

## hooks.rs

```rust
async fn init() -> Result {
    wisp::provide(Db::connect(&wisp::env("DB_URL").or_status(500)?).await?);
    Ok(())
}
fn before(cx: &mut Cx) -> Result {
    cx.cors("*")?;                    // a preflight is the Err that `?` returns
    Ok(())
}
```
No other `pub fn` here. Keep `before` sync: `async fn before` takes the
no-wait fast path off every route.

## API

- `cx`: `path() param(n) query(n) input(n) form() body() header(n) bearer()
  client_ip() method`, `query_or header_or cookie_or(n, d)`; `cookie
  set_cookie delete_cookie signed_cookie set_signed_cookie`; `sign_in
  sign_out signed_in user login signup`; `flash(msg) flashed() problem(n)`;
  `set(v) get::<T>() take::<T>()`; `fail(status, v) set_status set_header
  cors(o)`; `writes() need_bearer(env) need_signature(env, header)`.
- Errors: `error(404, "msg")`, `redirect("/x")`, `invalid("f", "msg")` return
  `Result`; `opt.or_404()?`, `.or_status(403)?`; `Error::new(s, m)`; any
  `std::error::Error` via `?` → 500.
- `Response::`: `json_of(&v) created(&v) text html empty(s) download(name,
  bytes) file_in(dir, name).await stream ndjson events websocket`
  + `.with_status(s) .with_header(n, v)`.
- State: `Table<T>`: `add(v)→id get(id) all() find(f) filter(f) update(id, f)
  remove(id) len() page(cx, 10)`; rows are `Row { id, value }` that read as
  the value. `Shared<T>` (`.lock()`), `wisp::provide(v)`/`state::<T>()`,
  `wisp::env("K")`, `spawn`, `every`, `wisp::channel("x")`
  `.send/events()` (SSE)`/websocket()`, `RateLimit::per_minute(n).check(key)?`,
  `#[derive(Cookie)]`.
- Data, files, jobs (docs/data.md): `Table::saved(n).unique("f", |v: &T| &v.f)
  .migrate(f).live()`, `set clear by try_add`; `Upload`, `wisp::relay`,
  `wisp::queue(n).push(&j)` + `work(n, f)` + `cron("0 3 * * *", f)`,
  `wisp::cache(k, secs, f)`/`uncache(path)`, `WISP_ADMIN_KEY` admin page;
  rules `url one_of pattern with`.
- Static export: `fn entries() -> Vec<&'static str>` in a `[param]` page.
  Test: `let mut app = wisp::test::client::<App>(); app.get("/").text()`,
  `app.post_form/post_json/delete`, `.json::<T>()`.

- Serve extras (docs/serve.md): embedded files gzip + `Range`; pages get
  `nosniff` and `referrer-policy` (`WISP_HSTS=on`, `WISP_SECURE_HEADERS=off`);
  `/_wisp/health`; `WISP_HANDLER_TIMEOUT=secs` → 503; `OTEL_EXPORTER_OTLP_ENDPOINT`.

## Gotchas

- `Err(error(..))` is wrong: `error()` already returns the `Result`.
- `+page.wisp` needs the `+`. Block statements and `fn load` are exclusive;
  a block's last statement ends with `;`.
- Don't hold `Shared::lock()` or other guards across `.await`; blocking work
  → `tokio::task::spawn_blocking` (a thread per core).
- `{#each x as y}` borrows a field path; `.iter()` other expressions.
- In `+server.rs`, a param named `id` (no `[id]` folder) serves `/[id]`: use
  `list` for the folder's GET. `#[validate]` on params is for actions.
- A field added to a saved type (`Rest`, `Table::saved`) must be `Option`,
  `Vec` or `bool`, so rows saved before it still read.

## Commands

`wisp new app` · `wisp dev` · `wisp check` · `wisp build` (`--static`,
`--docker`, `--target cloudflare|deno|vercel|netlify|node`, `--client ts`) ·
`wisp add|remove pkg`. Docs: README.md, docs/design.md, client.md, api.md,
deploy.md, embed.md, tokens.md.
