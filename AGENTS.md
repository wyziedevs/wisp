# Wisp: reference for coding agents

Wisp is a fast, fun web framework for Rust. File routes, `.wisp` templates
compiled to Rust, form actions, optional browser reactivity, one binary.

**Design rule (non-negotiable):** speed > flexibility > durability; developer
happiness last; app code in as few tokens as possible (AI writes most code,
so the cheapest, fastest, most flexible, durable framework wins). Wisp code:
Carmack style, minimal deps, no `unsafe` (but the Linux io_uring and epoll
drivers, `uring.rs` and `epoll.rs`, and the edge exports), no dead code,
zero warnings.

## Files

```
src/main.rs                 wisp::main!();   (generated; leave it)
src/app.html                shell with %wisp.head% %wisp.body% (optional)
src/app.css                 served at /_app/app.css; Tailwind if it imports it
src/hooks.rs                fn init() once; fn before(cx) every request
src/NAME.rs                 any module, no `mod` line: `NAME::f()` everywhere
src/components/Card.wisp    <Card title={x}>…</Card>
src/lib/*.js                browser modules, `import … from '$lib/x.js'`
src/params/word.rs          fn matches(s: &str) -> bool, for [x=word]
src/routes/…/+page.wisp     page: optional `---` Rust block, then markup
src/routes/…/+layout.wisp   wraps pages below; must <slot /> (or {@render children()})
src/routes/…/+error.wisp    error page; has `status`, `message`, `cx`
src/routes/…/+server.rs     endpoints: fn get/post/put/patch/delete/list
src/routes/…/+page.js       optional browser `load({data,url,params,fetch})`
static/…                    served at /
```

Folders: `blog` static, `[slug]` param, `[[lang]]` optional, `[...rest]`
rest, `[id=int]` digits only (u64), `[x=word]` custom matcher, `(group)` not
in URL. `+page.rs`/`+layout.rs` still work instead of a block (not both).

## A page

```html
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

let count = TODOS.len();
---
<title>Todos ({count})</title>
<form action="?/add">
  <input name="text">
</form>
{#each TODOS.all() as todo}
  <p>{todo} <button action="?/remove&id={todo.id}">x</button></p>
{/each}
```

Block rules:
- Items (`fn`, `struct`, `enum`, `use`, `static`, `const`, `impl`,
  `#[action]`) go in the page module. Statements are the page's load: run per
  request (GET, and after an action) before render; markup reads their names.
- Statements run in `async fn(cx: &mut Cx) -> Result`: `.await`, `?`,
  `return redirect("/x")`, `return error(404, "Gone")` work.
- Route params are locals: `slug: String`, `[id=int]` → `id: u64`,
  `[[lang]]` → `Option<String>`. Works even with no block: `<h1>{slug}</h1>`.
- No `use` lines needed: prelude = `Cx Response Result Error Json FromJson
  Rest Cookie Method Value Shared Table Row RateLimit OrStatus action error
  invalid redirect`.
  `Result` alone = `Result<()>`.
- Layout blocks: statements are sync, `cx: &Cx`, no `.await`/`?`.
- `const CACHE: u32 = 60;` (page or `+server.rs`): each worker keeps a GET's
  answer per host, path and query for 60 s and sends it as is (ETag, 304).
  Not for a request with a cookie or `authorization` (`CACHE_PUBLIC` is for
  all), nor an answer that sets a cookie or `cache-control: private` or
  `no-store`; not in dev. Hooks still run. The page must not read other
  headers.
- A page that reads nothing (no load, statements or params; literal holes,
  components with literal props) is baked whole at build: no `CACHE` needed.
- Old form: `+page.rs` with `struct Data {..}` + `fn load(..) -> Data`
  (markup reads Data fields by name, or `data.x`). A block may hold that too.

## Actions (form posts)

```rust
#[action]
fn like(id: u64, note: Option<String>, agree: bool, tags: Vec<String>) -> Result {
    cx.flash("Liked");                  // cx is added when the body uses it
    redirect("/")                       // 303; or Ok(()) to re-render the page
}
```

- `<form action="?/like">` posts to it (`method="post"` is added);
  `<form method="post">` with no `action` → `#[action] fn default`. `<button action="?/rm&id={x.id}">x</button>`
  outside a form is a one-button form.
- Params by name: route param, then form, then query. `T` required (400),
  `Option<T>` missing/blank → None, `bool` checkbox, `Vec<T>` repeated,
  `&str` ok. Returns `()`/`Result`, or `Response`/`Option<Response>` to send
  instead of the page.
- `#[validate(len = 1..=100)] text: String` (also `min max min_len max_len
  email`), or `return invalid("field", "msg")` → page re-renders as 422; each
  text `<input name>` of an action form (`?/x`, or `method="post"`) shows
  what was sent again and, after it, `<small class="problem">msg</small>`.
  Write `{cx.problem("field")}` anywhere to place them yourself (then none
  are added in that file). Other errors → error page.
- Same-origin checked. Works without JS; wisp.js morphs the page in place.

## Templates (Rust on the server)

| Syntax | Meaning |
|---|---|
| `{expr}` | escaped Display; an `Option` shows nothing for None |
| `{@html expr}` | raw (trusted only) |
| `attr={expr}` | quoted+escaped; `Option` → left out when None |
| `disabled={bool}` | boolean attrs present iff true |
| `<a {href}>` | `href={href}` |
| `class:on={bool}` | toggle class |
| `{#if c}…{:else if c}…{:else}…{/if}` | `if let Some(x) = y` works |
| `{#each list as item, i}…{:else}…{/each}` | for loop; `{:else}` when empty |
| `{#match e}{:case P}…{/match}` | match |
| `{@const x = expr}` | let |
| `{#snippet row(a, b)}…{/snippet}` `{@render row(x, 1)}` | local markup fn |
| `<head>…</head>` | into document head (also `<wisp:head>`); a top-level `<title>` goes there alone |
| `<slot />` or `{@render children()}` | layout/component slot |
| `cx` | the request (`&Cx`) in pages, layouts, error pages |

Holes can't go in `on*` attrs, tag names, `javascript:` URLs. `<script>` and
`<style>` contents are not parsed for holes.

## Components (`src/components/Name.wisp`)

```html
{@props title: &str, count: u32 = 0, featured: bool = false, row: Snippet<&Post, usize>}
<h2>{title}{#if featured} ★{/if}</h2>{@render children()}
```
Use: `<Card title={post.title} count={3} featured>kids</Card>`. Props are
typed and checked at build. No `---` block in components.

## Browser code (JavaScript, same file)

`{…}` is Rust on the server; quoted directive values and `{:…}` are JS.

```html
<button on:click="count++">Clicked {:count} times</button>
<script>
  let count = 0                       // top-level lets are state
  let big = $derived(count > 5)
  let name = data.name                // server values: data.x (a block's locals)
</script>
```

`bind:value="q"` with no `let q` anywhere declares it (state, starting from
the input), so a live search needs no script: `<input bind:value="q">`
`{:#each data.items.filter((i) => i.name.includes(q)) as i}…{:/each}`.

Directives: `on:click="f"` (modifiers `.prevent .stop .once .self .window
.document .outside .debounce.300ms .enter .escape .ctrl`…), `bind:value="q"`,
`bind:checked`, `bind:this="el"`, `:attr="js"`, `:text="js"`,
`class:x="js"`, `style:--x="js"`, `transition:fade|slide|scale|fly`,
`use:action="arg"`, `animate:flip`. Client blocks: `{:#if}…{:/if}`,
`{:#each items as it, i (it.id)}…{:/each}`, `{:@render s(x)}`. Runes:
`$state $state.raw $derived $effect $props $bindable $inspect`. Helpers (no
import): `onMount onDestroy effect watch tick listen goto invalidate`.
Stores in `src/lib`: `import { store, persisted, derived } from 'wisp'`.
Islands: `<Chart client:visible|idle|interaction|media="(…)"|none />`.
Server values sent to JS must `#[derive(Json)]`. Full: docs/client.md.

## Endpoints (`+server.rs`)

A whole JSON API, saved across restarts (`src/routes/api/notes/+server.rs`):

```rust
#[derive(Rest)]                    // Json + FromJson + Note::table()
#[rest(write = "API_KEY")]         // writes need Bearer $API_KEY; key = all, admin = DELETE
struct Note {
    #[validate(len = 1..=200)]     // len, min, max, min_len, max_len, email
    title: String,
    done: bool,                    // left out: false; Vec: []; Option: None
    tags: Vec<String>,
    created_at: String,            // set by Wisp (RFC 3339; u64 = unix seconds); also updated_at
}
fn before_create(note: &mut Note) -> Result { Ok(()) }  // also before_update(id, note),
fn after_update(note: &Row<Note>) {}                     // before_delete/after_*(row); cx optional
```
→ GET/POST `/api/notes`, GET/PUT/PATCH/DELETE `/api/notes/[id]`; rows are
`{"id":1,…}`; 201 + Location, 404, 422 by field. GET list:
`?done=true&title.has=tea&n.gte=2` (`.ne .gt .gte .lt .lte .has`),
`&sort=-created_at,title&limit=20&after=40` (or `&offset=`), `&fields=title`;
`x-total-count` + `link: rel="next"`; `accept: application/x-ndjson` = a row
a line. ETag on GET (304), `if-match` on writes (412); POST an array = bulk.
`#[rest(table = "n", ids = "random", memory)]`. In `users/[user]/notes`, a
`user` field is filtered and set from the route. A handler the file writes
replaces that one. Rows live in `WISP_DATA` log files (`.wisp/data` in dev);
`wisp::store(MyDb)` in `init` puts them in any DB (`impl wisp::Store`:
`load(table)`, `save(table, id, json)`). Edge: in memory. Handlers by hand:

```rust
fn list() -> Vec<Note> { db::all() }                  // GET /api/notes
fn post(body: New) -> Response { Response::created(&db::add(body)) }
fn get(id: u64) -> Option<Note> { db::find(id) }      // an `id` param → /api/notes/[id]
fn delete(id: u64) -> Option<()> { db::remove(id) }   // None → 404, Some(()) → 204
fn before(cx: &mut Cx) -> Result {                    // runs before each handler here
    if cx.writes() { cx.need_bearer("API_KEY")?; }    // 401 unless Bearer $API_KEY
    Ok(())
}
```

`body: T` = whole JSON body (422 with `errors` by field). Other params by
name as for actions. Returns: value (`#[derive(Json)]`) → 200 JSON; nothing →
204; `Response`; `Option<T>` (None → 404). Errors of endpoints, `/api` and
JSON requests are JSON `{"status","code","error","errors"}` (`code`:
`not_found` `invalid`…, or `Error::new(409, "x").with_code("taken")`);
`accept: application/problem+json` or `WISP_PROBLEM_JSON=on` → RFC 9457.
POST with `Idempotency-Key` replays the first answer. Webhooks:
`cx.need_signature("GITHUB_SECRET", "x-hub-signature-256")?` (hex, base64,
Stripe). Big lists: `Response::ndjson(|out| async move { out.line(&x).await?; Ok(()) })`.
Versions are folders (`api/v1`). `const BODY_LIMIT: usize = 20 * wisp::MB;`.
OpenAPI at `/_wisp/docs`; TypeScript client at `/_wisp/client.ts` or
`wisp build --client ts`.
## hooks.rs

```rust
async fn init() -> Result {
    wisp::provide(Db::connect(&wisp::env("DB_URL").or_status(500)?).await?);
    Ok(())
}
fn before(cx: &mut Cx) -> Result<Option<Response>> {
    if let Some(r) = cx.cors("*") { return Ok(Some(r)); }
    if cx.writes() && cx.path().starts_with("/api") {
        cx.need_bearer("API_KEY")?;
    }
    Ok(None)
}
```
Any other `pub fn` in hooks.rs is an error. `pub` types there are
`crate::hooks::T`.

## API

- `cx`: `path() param(n) query(n) query_or(n, d) input(n) problem(n) form()
  .get/.file/.files body() header(n) header_or(n, d) bearer() need_bearer(env)
  writes() need_signature(env, header)
  basic_auth() cookie(n)
  cookie_or(n, d) set_cookie(n, v) delete_cookie(n) signed_cookie(n)
  set_signed_cookie(n, v) flash(msg) flashed() set(v) get::<T>() take::<T>()
  fail(status, v) set_status(s) set_header(n, v) client_ip() cors(o)
  method`.
- Errors: `error(404, "msg")`, `redirect("/x")`, `invalid("f", "msg")` return
  `Result`; `opt.or_404()?`, `.or_status(403)?`; `Error::new(s, m)` for
  `map_err`; any `std::error::Error` via `?` → 500.
- `Response::`: `json_of(&v) created(&v) text html empty(s)
  download(name, bytes) file_in(dir, name).await stream ndjson events
  websocket` + `.with_status(s) .with_header(n, v)`.
- State: `Table<T>` (`Table::new()` memory, `Table::saved("name")` survives
  restarts; `add(v)→id get(id) all() find(f) filter(f) update(id, f)
  remove(id) len()`, rows are `Row { id, value }` that read as the value),
  `Shared<T>` (`.lock()`), `wisp::provide(v)` / `wisp::state::<T>()`,
  `wisp::env("K")`, `wisp::env_or("K", d)`, `wisp::spawn`, `wisp::every`,
  `wisp::channel("x").send/subscribe/connect`, `RateLimit::per_minute(n)
  .check(key)?`, `#[derive(Cookie)]`, `#[derive(Json)]`.
- Static export: `fn entries() -> Vec<&'static str>` in a `[param]` page's
  block. Test: `let mut app = wisp::test::client::<App>(); app.get("/").text()`,
  `app.bearer(t) app.header(n, v) post_json put_json patch_json delete`,
  `.json::<T>()`; tables are in memory in tests.

## Gotchas

- `Err(error(..))` is wrong: `error()` already returns the `Result`.
- `+page.wisp` needs the `+`. A page can't have both a block and `+page.rs`.
- Block statements and `fn load` are exclusive. A block's last statement
  ends with `;`.
- An action returns nothing, `Result`, or a response, never data.
- Don't hold `Shared::lock()` or other guards across `.await`.
- `{#each x as y}` borrows a field path; call `.iter()` on other expressions.
- Handlers run on a thread per core: blocking work → `tokio::task::spawn_blocking`.
- In JS, `data.x` reads server values; a name both Rust and JS is an error.
- In `+server.rs`, a param named `id` (the folder having no `[id]`) serves
  `/[id]`: use `list` for the folder's GET. `#[validate]` on params is for
  actions; endpoints validate their `body: T` type's fields.
- A field added to a saved type (`#[derive(Rest)]`, `Table::saved`) must be
  `Option`, `Vec` or `bool`, so rows saved before it still read.

## Commands

`wisp new app` · `wisp dev` (hot reload) · `wisp check` · `wisp build`
(`--static`, `--docker`, `--target cloudflare|deno|vercel|netlify|node`,
`--client ts`).
Docs: README.md, docs/design.md, docs/client.md, docs/api.md,
docs/deploy.md, docs/embed.md, docs/tokens.md.
