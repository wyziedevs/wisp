# Wisp: reference for coding agents

Wisp is a fast, fun web framework for Rust. File routes, `.wisp` templates
compiled to Rust, form actions, optional browser reactivity, one binary.

**Design rule (non-negotiable):** speed > flexibility > durability; developer
happiness last; app code in as few tokens as possible (AI writes most code,
so the cheapest, fastest, most flexible, durable framework wins). Wisp code:
Carmack style, minimal deps, no `unsafe`, no dead code, zero warnings.

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
src/routes/…/+layout.wisp   wraps pages below; must {@render children()}
src/routes/…/+error.wisp    error page; has `status`, `message`, `cx`
src/routes/…/+server.rs     endpoints: fn get/post/put/patch/delete
src/routes/…/+page.js       optional browser `load({data,url,params,fetch})`
static/…                    served at /
```

Folders: `blog` static, `[slug]` param, `[[lang]]` optional, `[...rest]`
rest, `[id=int]` digits only (u64), `[x=word]` custom matcher, `(group)` not
in URL. `+page.rs`/`+layout.rs` still work instead of a block (not both).

## A page

```html
---
static TODOS: Shared<Vec<String>> = Shared::new(Vec::new());

#[action]
fn add(text: String) -> Result {
    if text.trim().is_empty() {
        return invalid("text", "Write something");
    }
    TODOS.lock().push(text);
    Ok(())
}

let count = TODOS.lock().len();
---
<head><title>Todos ({count})</title></head>
<form method="post" action="?/add">
  <input name="text" value={cx.input("text")}>
  {#if let Some(p) = cx.problem("text")}<p>{p}</p>{/if}
</form>
{#each TODOS.lock().iter() as todo, i}<p>{i}: {todo}</p>{/each}
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
  Cookie Method Value Shared RateLimit OrStatus action error invalid redirect`.
  `Result` alone = `Result<()>`.
- Layout blocks: statements are sync, `cx: &Cx`, no `.await`/`?`.
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

- `<form method="post" action="?/like">`; no `action` → `#[action] fn default`.
- Params by name: route param, then form, then query. `T` required (400),
  `Option<T>` missing/blank → None, `bool` checkbox, `Vec<T>` repeated,
  `&str` ok. Returns `()`/`Result`, or `Response`/`Option<Response>` to send
  instead of the page.
- `return invalid("field", "msg")` → page re-renders as 422 with
  `cx.problem("field")`, `cx.input("field")`. Other errors → error page.
- Same-origin checked. Works without JS; wisp.js morphs the page in place.

## Templates (Rust on the server)

| Syntax | Meaning |
|---|---|
| `{expr}` | escaped Display |
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
| `<head>…</head>` | into document head (also `<wisp:head>`) |
| `{@render children()}` | layout/component slot |
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

```rust
#[derive(FromJson)]
struct New {
    #[validate(min_len = 1, max_len = 200)]  // min, max, min_len, max_len, email
    title: String,
}

fn get(id: u64) -> Result<Note> { notes::find(id).or_404() }   // value → JSON
fn post(body: New) -> Response { Response::created(&notes::add(body.title)) }
fn delete(id: u64) -> Result { notes::remove(id).or_404() }    // Option<()>: 204 or 404
```

`body: T` = whole JSON body (422 with `errors` by field). Other params by
name as for actions. Errors under `/api` or JSON requests are JSON.
Returns: value (`#[derive(Json)]`) → 200 JSON; nothing → 204; `Response`;
`Option<Response>` (None → 404). `const BODY_LIMIT: usize = 20 * wisp::MB;`.
OpenAPI at `/_wisp/docs`.

## hooks.rs

```rust
async fn init() -> Result {
    wisp::provide(Db::connect(&wisp::env("DB_URL").or_status(500)?).await?);
    Ok(())
}
fn before(cx: &mut Cx) -> Result<Option<Response>> {
    if let Some(r) = cx.cors("*") { return Ok(Some(r)); }
    if cx.method != Method::Get && cx.bearer() != wisp::env("API_KEY").as_deref() {
        return error(401, "Unauthorized");
    }
    Ok(None)
}
```
Any other `pub fn` in hooks.rs is an error. `pub` types there are
`crate::hooks::T`.

## API

- `cx`: `path() param(n) query(n) query_or(n, d) input(n) problem(n) form()
  .get/.file/.files body() header(n) bearer() basic_auth() cookie(n)
  cookie_or(n, d) set_cookie(n, v) delete_cookie(n) signed_cookie(n)
  set_signed_cookie(n, v) flash(msg) flashed() set(v) get::<T>() take::<T>()
  fail(status, v) set_status(s) set_header(n, v) client_ip() cors(o)
  method`.
- Errors: `error(404, "msg")`, `redirect("/x")`, `invalid("f", "msg")` return
  `Result`; `opt.or_404()?`, `.or_status(403)?`; `Error::new(s, m)` for
  `map_err`; any `std::error::Error` via `?` → 500.
- `Response::`: `json_of(&v) created(&v) text html redirect empty(s)
  download(name, bytes) file_in(dir, name).await stream events
  websocket` + `.with_status(s) .with_header(n, v)`.
- State: `Shared<T>` (`.lock()`), `wisp::provide(v)` / `wisp::state::<T>()`,
  `wisp::env("K")`, `wisp::env_or("K", d)`, `wisp::spawn`, `wisp::every`,
  `wisp::channel("x").send/subscribe/connect`, `RateLimit::per_minute(n)
  .check(key)?`, `#[derive(Cookie)]`, `#[derive(Json)]`.
- Static export: `fn entries() -> Vec<&'static str>` in a `[param]` page's
  block. Test: `let mut app = wisp::test::client::<App>(); app.get("/").text()`.

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

## Commands

`wisp new app` · `wisp dev` (hot reload) · `wisp check` · `wisp build`
(`--static`, `--docker`, `--target cloudflare|deno|vercel|netlify|node`).
Docs: README.md, docs/design.md, docs/client.md, docs/api.md,
docs/deploy.md, docs/embed.md, docs/tokens.md.
