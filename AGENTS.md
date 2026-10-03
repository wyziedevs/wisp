# Wisp: reference for coding agents

Wisp is a fast, fun web framework for Rust. File routes, `.wisp` templates
compiled to Rust, form actions, optional browser reactivity, one binary.

<!-- repo: this part is for work on Wisp itself; `wisp new` leaves it out -->
**Design rule (non-negotiable), in order:** 1. ultra fast, 2. cheap (fewest
tokens to write app code: AI writes most code), 3. durable (every fast path
proven at startup with a fallback; nothing after startup can take the process
down), 4. flexible. Developer happiness last. Wisp code:
Carmack style, minimal deps, no `unsafe` (but the Linux io_uring and epoll
drivers, `uring.rs` and `epoll.rs`, and the edge exports), no dead code,
zero warnings. Apps get this file without this part (`wisp new`, `wisp
update-docs`); `llms-full.txt` is made from it and docs/ by a test.
<!-- /repo -->

## Files

```
src/main.rs                 wisp::main!();   (generated; leave it)
src/app.html                shell with %wisp.head% %wisp.body% (optional)
src/app.css                 served at /_app/app.css; Tailwind if it imports it
src/app.scss                Sass instead of app.css, served the same (no Node)
postcss.config.*            PostCSS after either (needs Node + node_modules/postcss-cli)
package.json                npm packages for browser code: `wisp add canvas-confetti`, no Node
.env                        X=…: `wisp::env("X")` (start), `env.PUBLIC_X` in browser code (build)
src/hooks.rs                fn init() once; fn before(cx) every request
src/NAME.rs                 any module, no `mod` line: `NAME::f()` everywhere
src/components/Card.wisp    <Card title={x}>…</Card>
src/lib/*.js (or .ts)       browser modules, `import … from '$lib/x.js'` (or `'$lib/x'`)
src/params/word.rs          fn matches(s: &str) -> bool, for [x=word]
src/routes/…/+page.wisp     page: optional `---` Rust block, then markup
src/routes/…/+layout.wisp   wraps pages below; must <slot /> (or {@render children()})
src/routes/…/+error.wisp    error page; has `status`, `message`, `cx`
src/routes/…/+server.rs     endpoints: fn get/post/put/patch/delete/list
src/routes/…/+page.js       optional browser `load({data,url,params,fetch})` (or +page.ts)
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
- No `use` lines needed: prelude = `Cx Response Result Error Email Image Json
  FromJson Rest Cookie Method Value Shared Table Row RateLimit OrStatus KB MB
  action error invalid redirect`.
  `Result` alone = `Result<()>`.
- Layout blocks: statements are sync, `cx: &Cx`, no `.await`/`?`.
- `const CACHE: u32 = 60;` (page or `+server.rs`): each worker keeps a GET's
  answer per host, path and query for 60 s and sends it as is (ETag, 304).
  Not for a request with a cookie or `authorization` (`CACHE_PUBLIC` is for
  all), nor an answer that sets a cookie or `cache-control: private` or
  `no-store`; not in dev. Hooks still run. The page must not read other
  headers.
- `.await` in a page's markup, outside any block, runs with the statements
  before render: `{#each db::items().await as item}` needs no block. Not
  inside `{#if}`/`{#each}` bodies, layouts or components (build error).
- A page that reads nothing (no load, statements or params; literal holes,
  components with literal props) is baked whole at build: no `CACHE` needed.
- Old form: `+page.rs` with `struct Data {..}` + `fn load(..) -> Data`
  (markup reads Data fields by name, or `data.x`). A block may hold that too.

## Actions (form posts)

```rust
#[action]
fn like(id: u64, email: Email, note: Option<String>, agree: bool, tags: Vec<String>) {
    cx.flash("Liked");                  // cx is added when the body uses it
    redirect("/")                       // 303; or end in `;` to re-render the page
}
```

- No `->`: the action returns `Result`, so it may use `?`, `return
  error(..)`, `return;`, and end in `redirect(..)`/`invalid(..)` or `;`.
  No `async` either: a body with `.await` makes it async.

- `<form action="?/like">` posts to it (`method="post"` is added);
  `<form method="post">` with no `action` → `#[action] fn default`. `<button action="?/rm&id={x.id}">x</button>`
  outside a form is a one-button form.
- Params by name: route param, then form, then query. `T` required (400
  missing or a body that isn't JSON; sent but not a `T` → 422 by field,
  like `invalid`), `Option<T>`
  missing/blank → None, `bool` checkbox, `Vec<T>` repeated, `&str` ok,
  `Email` (what `<input type=email>` takes), a `#[derive(FromJson)]`/`Rest` struct (the page's or `src/*.rs`'s)
  (`fn default(post: Post)`: its fields by name, its `#[validate]`s; blank
  = missing). Returns nothing/`Result`, or `Response`/`Option<Response>`
  to send instead of the page.
- Uploads: `#[validate(max_size = 1 * MB)] avatar: Image` (`Option<Image>`
  may be empty) in a form with `enctype="multipart/form-data"`. Only PNG,
  JPEG, GIF, WebP, AVIF by their bytes (not SVG); else 422 by field. The
  body limit grows by `max_size` itself: no `BODY_LIMIT`. Store it in a
  table field (`avatar: Option<Image>`), serve it with
  `fn get(id: u64) -> Option<Image> { USERS.get(id)?.value.avatar }` in
  `avatars/[id=int]/+server.rs` (type, ETag/304, `no-cache`).
- `#[validate(len = 1..=100)] text: String` (also `min max min_len max_len
  email`), or `return invalid("field", "msg")` → page re-renders as 422
  listing every failing field. Its inputs get the browser's own checks
  that match (`required`, `minlength`, `type="email"`, `min`/`max` on
  `type="number"`, a `pattern` for the most length); the server still
  checks all; a button with `formaction="?/other"` skips them
  (`formnovalidate`). Each named `<input>`, `<textarea>`, `<select>` of an
  action form (`?/x`, or `method="post"`) shows what was sent again, else
  its own value (`value={post.title}` or `value="x"`, not `value="a{b}"`,
  `<textarea name="body">{post.body}</textarea>`, `<select name="kind"
  value={post.kind}>` selects the option of that value, or text), and after it `<small
  class="problem">msg</small>` (passwords/files: the problem, never the
  value). `{cx.problem("field")}` puts that field's `<small>` there instead.
  Other errors → error page.
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

`<style>h1 { color: red }</style>` (no attributes, top level) styles this
file's elements only: each gets a `w-xxxxxx` class, each selector's last
compound too. `:global(body)` opts out; `<style global>` stays as written.
The CSS joins `/_app/app.css`. `@import` goes in `src/app.css`.

Accessibility lints warn (check, dev, build), never fail: img without alt,
on:click on a non-interactive element without role + on:keydown, label
without control or `for`, `<a>` without href or `href="#"`, autofocus,
skipped heading levels, button without text/aria-label, tabindex > 0,
unknown `aria-*`. `<!-- wisp-ignore a11y-img-alt -->` on the line before
silences one. Fix them rather than silence them.

Holes can't go in `on*` attrs, tag names, `javascript:` URLs, SVG
`<animate>`/`<set>` `to`/`from`/`values`/`by`, or `<meta http-equiv>`/refresh
`content`, in any case; nor can `{:…}`/`:attr`, and `{:...obj}` leaves
those keys out. `<script>` and `<style>` contents are not parsed for holes.

## Components (`src/components/Name.wisp`)

```html
{@props title: &str, count: u32 = 0, featured: bool = false}
<h2>{title}{#if featured} ★{/if} ({count})</h2>{@render children()}
```
Use: `<Card title={post.title} count={3} featured>kids</Card>`. Props are
typed and checked at build; one without a default is required. No `---`
block in components. Markup as a prop: `row: Snippet<&Post, usize>`,
shown with `{@render row(p, i)}`, given as `<Table {row} />` or a
`{#snippet row(p, i)}…{/snippet}` among the tag's children.

## Browser code (JavaScript, same file)

`{…}` is Rust on the server; quoted directive values and `{:…}` are JS.

```html
<button on:click="count++">Clicked {:count} times</button>
<script>
  let count = 0                       // top-level lets are state
  let big = $derived(count > 5)
  let name = user.name                // a block's `let user` (or `data.user`)
</script>
```

A page's (or layout's) Rust names are browser values by name: `items` is
the block's `items` (also `data.items`); a name the script declares is the
script's (`let guess = data.guess`). `bind:value="q"` with no `let q`
anywhere declares it (state), so a live search needs no script:
`<input bind:value="q">` `{:#each items.filter((i) => matches(i.name, q)) as i}…{:/each}`
(`matches(text, q)`: case-blind contains; empty `q` matches).

Directives: `on:click="f"` (modifiers `.prevent .stop .once .self .window
.document .outside .debounce.300ms .enter .escape .ctrl`…), `bind:value="q"`,
`bind:checked`, `bind:this="el"`, `:attr="js"` (`:hidden="!open"` with
`let open = false` is rendered hidden: no static `hidden`), `:text="js"`,
`class:x="js"`, `style:--x="js"`, `transition:fade|slide|scale|fly`,
`use:action="arg"`, `animate:flip`. Client blocks: `{:#if}…{:/if}`,
`{:#each items as it, i (it.id)}…{:/each}`, `{:@render s(x)}`. Runes:
`$state $state.raw $derived $effect $props $bindable $inspect`. Helpers (no
import): `onMount onDestroy effect watch tick listen goto invalidate matches`.
Shallow routing (tabs, modals): `pushState('?tab=2', { tab: 2 })`,
`replaceState('', s)`; `page.value.state` is the entry's, reactive; back and
forward restore it with no request.
Stores in `src/lib`: `import { store, persisted, derived } from 'wisp'`.
npm: `wisp add canvas-confetti`, then `import confetti from 'canvas-confetti'`
(esm.sh in dev; `wisp build` puts it in the binary, no CDN; one not in package.json
is a build error).
Islands: `<Chart client:visible|idle|interaction|media="(…)"|none />`.
Server components: one with no browser code ships no JS; islands and server
components nest in any order (an inner island wakes a waiting outer one).
TypeScript: `<script lang="ts">`, `src/lib/*.ts`, `+page.ts`. Types are
stripped (spaces, so lines and columns hold), as Node's strip-types does:
`enum`, a `namespace` with values and parameter properties are build errors
(use `as const` objects, modules, fields). `wisp check --types` runs the
app's tsc (`npm i -D typescript`, or `WISP_TSC`) with server values typed
by the compiler (`let items = vec![Item{..}]` → `Item[]`; a type with no TS
→ `unknown`, noted).
Env: `env.PUBLIC_API_URL` in browser code (scripts, directives, `src/lib`,
`+page.js`) is filled in at build from the process's `PUBLIC_*`, else
`.env`'s (dev rebuilds when it changes). `env.SECRET` (no `PUBLIC_`), a
`PUBLIC_*` that is not set, or `env` read whole is a build error. The
server's `wisp::env("X")` reads the process's environment, else `.env`'s
(once, at start; a bad line warns; not on edge hosts).
Source maps: dev serves `/_app/c/t3.js.map` beside each module (the `.wisp`
line of each line); release only with `wisp build --sourcemap` (`--static`
writes them too).
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
`load(table)`, `save(table, id, json)`). Edge: in memory, or env
`WISP_STORE=d1:DB|deno-kv|libsql://…` (+`WISP_STORE_TOKEN`). Handlers by hand:

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
POST with `Idempotency-Key` replays the first answer (per cookie). Webhooks:
`cx.need_signature("GITHUB_SECRET", "x-hub-signature-256")?` (hex, base64,
Stripe). Big lists: `Response::ndjson(|out| async move { out.line(&x).await?; Ok(()) })`.
Versions are folders (`api/v1`). `const BODY_LIMIT: usize = 20 * wisp::MB;`.
OpenAPI at `/_wisp/docs`; TypeScript client at `/_wisp/client.ts` or
`wisp build --client ts`.
## Members

```rust
static USERS: Table<User> = Table::saved("users");   // User { name, hash: String, .. }

#[action]
fn join(name: String, #[validate(min_len = 8)] password: String) {
    let id = USERS.add(User { name, hash: wisp::password::hash(&password).await? });
    cx.sign_in(id);                     // signed cookie, 30 days, a new one
    redirect("/me")
}
#[action]
fn login(name: String, password: String) {
    let user = USERS.find(|u| u.name == name);
    let hash = user.as_ref().map(|u| u.hash.as_str());  // None: as slow, so names stay secret
    if !wisp::password::check(&password, hash).await? { return invalid("password", "Wrong name or password"); }
    cx.sign_in(user.unwrap().id);
    redirect("/me")
}
let me = cx.user(&USERS)?;              // a members' page: Row<User>, or 303 to /login
```
`cx.signed_in()?` is the id alone; `.ok()` asks without redirecting;
`cx.sign_out()`; `wisp::sign_out_everywhere(id)?` ends all of `id`'s
sessions, stolen ones too (saved; read at start; with a shared
`wisp::store` other instances see it within 30 s, log files are per
instance). Endpoints/JSON clients get 401. Sign-in page elsewhere:
`wisp::sign_in_page("/enter")` in `init`. Hashes are PBKDF2-SHA256,
600,000 rounds (~0.2 s of a core, on purpose, off the worker: `.await?`
them; 503 + `retry-after` when ~3 s are queued; `RateLimit` sign-in against floods). `wisp::password::outdated(&hash)` → rehash at
sign-in. Rotate the secret: new `WISP_SECRET`, old one in `WISP_SECRET_OLD`
for 30 days (sessions' life; 400 for other signed cookies), then drop it.

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
Pages get a `content-security-policy` (`'self'`, inline template scripts
by build-time hash, `img-src 'self' data: https:`, inline styles ok; dev
adds esm.sh). `wisp::csp("img-src 'self' https://cdn.x")` in `init`
replaces that directive (or adds one); `wisp::csp_off()` sends none.
`onclick="…"` and scripts in `{@html}` don't run: use `on:click` or a file.
Any other `pub fn` in hooks.rs is an error. `pub` types there are
`crate::hooks::T`. Keep `before` sync: an `async fn before`
takes the no-wait fast path off every route.

## API

- `cx`: `path() param(n) query(n) query_or(n, d) input(n) problem(n) form()
  .get/.file/.files body() header(n) header_or(n, d) bearer() need_bearer(env)
  writes() need_signature(env, header)
  basic_auth() cookie(n)
  cookie_or(n, d) set_cookie(n, v) delete_cookie(n) signed_cookie(n)
  set_signed_cookie(n, v) sign_in(id) sign_out() signed_in() user(&TABLE)
  flash(msg) flashed() set(v) get::<T>() take::<T>()
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
  remove(id) len()`, rows are `Row { id, value }` that read as the value;
  `page(cx, 10)`: `?page=N`'s rows newest first, `{#each posts as p}`,
  `{#if let Some(href) = posts.next}<a {href}>Older</a>{/if}`, also `prev`),
  `Shared<T>` (`.lock()`), `wisp::provide(v)` / `wisp::state::<T>()`,
  `wisp::env("K")`, `wisp::env_or("K", d)`, `wisp::spawn`, `wisp::every`,
  `wisp::channel("x").send/subscribe/connect`, `fn get() -> Response {
  wisp::channel("x").events() }` (SSE) or `.websocket()`, `RateLimit::per_minute(n)
  .check(key)?`, `#[derive(Cookie)]`, `#[derive(Json)]`.
- Static export: `fn entries() -> Vec<&'static str>` in a `[param]` page's
  block.
- Tests (`cargo test`; in `src/tests.rs` with `use crate::App;`, or
  `tests/x.rs` after `wisp::app!();`): `let mut app =
  wisp::test::client::<App>(); app.get("/").text()`, `app.post_form("/?/add",
  &[("text", "hi")]) bearer(t) header(n, v) post_json put_json patch_json
  delete cookie(n)`, `.status`, `.json::<T>()`; cookies kept; tables are in
  memory.
- Browser test (`wisp test --browser` = `cargo test --features browser`, headless Chrome/Edge;
  passes, skipped, without one): `let mut b = wisp::browser!(App);
  b.goto("/"); b.click("text=Plus One"); assert_eq!(b.text("output"), "1");`
  also `hover fill(sel, t) press("Enter") attr(sel, n) count wait
  eval(js) -> Value url screenshot(path)`. Selectors: CSS or `text=…`
  (text, aria-label or title). Actions wait for the element and for the
  page to settle (5 s, `b.timeout(d)`): no sleeps needed.

## Gotchas

- `Err(error(..))` is wrong: `error()` already returns the `Result`.
- `+page.wisp` needs the `+`. A page can't have both a block and `+page.rs`.
- Block statements and `fn load` are exclusive. A block's last statement
  ends with `;`.
- An action returns nothing, `Result`, or a response, never data.
- Don't hold `Shared::lock()` or other guards across `.await`.
- `{#each x as y}` borrows a field path; call `.iter()` on other expressions.
- Handlers run on a thread per core: blocking work → `tokio::task::spawn_blocking`.
- In a page's JS, a Rust name reads its server value (`data.x` too); a
  browser global (`document`, `location`, `event`…) stays the browser's. In
  a component, a prop the script also declares is an error.
- An action without `->` must end in `;` or a `Result`, not another value.
- In `+server.rs`, a param named `id` (the folder having no `[id]`) serves
  `/[id]`: use `list` for the folder's GET. `#[validate]` on params is for
  actions; endpoints validate their `body: T` type's fields.
- A field added to a saved type (`#[derive(Rest)]`, `Table::saved`) must be
  `Option`, `Vec` or `bool`, so rows saved before it still read.

## Commands

`wisp new app` · `wisp dev` (hot reload) · `wisp test [--browser] [args]` · `wisp check [--types]` · `wisp fmt [paths]`
(`--check`, `--stdin`; markup, `---` via rustfmt, scripts, styles) · `wisp build`
(`--static`, `--docker`, `--target cloudflare|deno|vercel|netlify|node|bun|lambda|native`,
`--client ts`, `--sourcemap`; in a host's CI it picks that host) · `wisp deploy init <host>`
(GitHub Actions) · `wisp add pkg[@ver]` · `wisp remove pkg` · `wisp lsp` (language
server; setup per editor: editors/README.md) · `wisp update-docs` (this file, after
upgrading Wisp) · `wisp mcp` (tools for AI agents: `wisp_docs(topic)`,
`wisp_check`, `wisp_routes`, `wisp_components`, `wisp_new_route(path, kind)`;
Claude Code: `claude mcp add wisp -- wisp mcp`). More: `wisp_docs`, or
https://raw.githubusercontent.com/wyziedevs/wisp/main/llms-full.txt (this
file and every doc).
Hot reload keeps state: a saved `.wisp` whose script or `{:…}` markup
changed swaps its module in place, in ms, no compile: `$state` kept by
name, focus, selection and fields kept; text alone morphs that file's part
of the page; a `<style>` swaps the stylesheet. Rust (`---`, `{expr}`,
`{@props}`) compiles; a changed `---` or `{@props}`, a top-level statement
other than declarations/helpers (`init()`, `if`, `window.x =`) or a swap
that throws starts it afresh, with one console line saying why.
Under `wisp dev` (debug builds only): `Alt+Shift+W` opens the devtools
(components, editable `$state`, props, stores, route, timings, open in
editor via `$WISP_EDITOR`/`$EDITOR`/`code -g`); `/_wisp/components` is the
component workshop: `src/components/Card.stories.wisp` holds
`{#story "Featured"}<Card featured title="x" />{/story}` blocks, each
rendered on its own page with controls for its `&str`/`String`, number
and `bool` props; a component without one gets a default story when all
its required props are of those types. Release builds ignore story files.
