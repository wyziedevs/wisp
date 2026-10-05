# Wisp: reference for coding agents

Wisp is a fast, fun web framework for Rust. File routes, `.wisp` templates
compiled to Rust, form actions, optional browser reactivity, one binary.
This file is the short reference, most-used first; each section ends with
the docs page that has the rest (https://wispweb.dev/docs, or `wisp mcp`).

<!-- repo: this part is for work on Wisp itself; `wisp new` leaves it out -->
**Design rule (non-negotiable), in order:** 1. ultra fast, 2. cheap (fewest
tokens to write app code: AI writes most code), 3. durable (every fast path
proven at startup with a fallback; nothing after startup can take the process
down), 4. flexible. Developer happiness last. Wisp code:
Carmack style, minimal deps, no `unsafe` (but the Linux io_uring and epoll
drivers, `uring.rs` and `epoll.rs`, and the edge exports), no dead code,
zero warnings. Apps get this file without this part (`wisp new`, `wisp
update-docs`); `llms-full.txt` is made from it and the docs site pages (checkout `../wisp-docs`) by a test.
<!-- /repo -->

## Files

```
src/main.rs                 wisp::main!();   (generated; leave it)
src/routes/…/+page.wisp     page: optional `---` Rust block (load, actions, `mod server` endpoints), then markup
src/routes/…/+layout.wisp   wraps pages below; must <slot /> (or {@render children()})
src/routes/…/+server.rs     endpoints: fn get/post/put/patch/delete/list (or `mod server {}` in the page's block)
src/routes/…/+error.wisp    error page; has `status`, `message`, `cx`
src/routes/…/+page@.wisp    a page without the layouts above it (`+page@app.wisp`: only up to the `(app)` layout)
src/routes/…/+loading.wisp  static HTML a client navigation shows in <main> while a page below loads
src/routes/…/+page.md       Markdown page (`blog/x.md` = /blog/x)
src/routes/…/+page.js       optional browser `load({data,url,params,fetch})` (or .ts)
src/db.rs                   models and tables
src/NAME.rs                 any module, no `mod` line; its `pub` items need no `use` anywhere
src/components/Card.wisp    <Card title={x}>…</Card>
src/hooks.rs                fn init() once; fn before(cx) every request
src/middleware.rs           named middleware: `pub fn auth(cx: &mut Cx) -> Result`, used by `const MIDDLEWARE`
src/remote.rs               #[remote] fns browser code calls (or in a page's block)
src/lib/*.js (or .ts)       browser modules, `import … from '$lib/x.js'`
src/app.html                shell with %wisp.head% %wisp.body% (optional)
src/app.css | app.scss      served at /_app/app.css (Tailwind if it imports it; Sass, no Node)
src/params/word.rs          fn matches(s: &str) -> bool, for [x=word]
src/locales/en.json         messages, fr.json etc.: {t("key")}
src/fonts.txt               a font a line: `Inter inter.woff2 100-900` (static/fonts) or `Open_Sans google 400 700`; CSS `var(--font-inter)`
src/manifest.json           web app manifest: {"name": "Notes", "offline": true}
static/…                    served at /
.env                        X=…: `wisp::env("X")`, `env.PUBLIC_X` in browser code
package.json                npm packages for browser code: `wisp add canvas-confetti`
```

Folders: `blog` static, `[slug]` param, `[[lang]]` optional, `[...rest]`
rest, `[id=int]` digits (u64), `[x=word]` custom matcher, `[[lang=locale]]`
one of `src/locales`, `(group)` not in URL; a param is not `cx` or `__x`.
`+page.rs` (`struct Data` + `fn load(..) -> Data`, read by name) and
`+layout.rs` work instead of a block. Slots (`dash/@stats/+page.wisp` drawn
by the layout's `{@render stats()}`) and intercepting routes
(`feed/@modal/(.)photo/[id]/+page@.wisp`): https://wispweb.dev/docs/design-features.
`/sitemap.xml` (pages without params, or with `entries()`; not `(private)`
groups or `noindex` pages; host from env `SITE_URL`) and `/robots.txt` are
made; a route or `static/` file of that name wins.

## A page

A route is one file: the `---` block holds its Rust (load, actions, and
endpoints in `mod server`), the markup follows. `+page.rs` and `+server.rs`
beside it build the same, for a page whose Rust outgrows the block.

```rust
// src/db.rs
#[model] // Json + FromJson + Clone, all pub
struct Todo {
    #[validate(len = 1..=100)]
    text: String,
}
pub static TODOS: Table<Todo> = Table::saved(); // "todos"; Table::new() = memory
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
<form action="?/add" fields />
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
  FromJson Rest Config Cookie CookieOptions SameSite Method Value Shared Table Row RateLimit OrStatus Password Reply KB MB
  action remote error invalid model redirect Always Never Ignore`, the `pub` items of
  `src/*.rs` and of modules `main.rs` declares, `HashMap HashSet BTreeMap BTreeSet
  VecDeque Arc Rc Cow Duration Instant SystemTime`, and `[package.metadata.wisp]
  auto = ["chrono::{Utc, DateTime}"]`. The file's own items and `use` lines
  win; a name two modules share is a build error (write `db::Post`).
  `wisp check --explain-imports` lists them. `Result` alone = `Result<()>`.
- `Value` (any JSON): `get(k)`, `as_str() as_bool() as_f64()`.
- `.await` in markup outside any block runs with the statements:
  `{#each items().await as item}` (not in bodies, layouts, components). A
  page that reads nothing is baked at build.
- Knobs, `const` in the block (or `+server.rs`); a route that sets none pays nothing:
  `CACHE: u32 = 60` keeps a GET 60 s per worker (ETag, 304; never with a cookie
  or `authorization`, `CACHE_PUBLIC` all; not in dev), `CACHE_STALE: u32 = 600`
  serves it stale while one request renews, `CACHE_TAGS: &[&str] = &["posts"]`
  + `wisp::revalidate_tag("posts")` drops by tag (`cx.cache_tag(t)`,
  `cx.enter_draft()`/`exit_draft()`/`draft()` bypass a public one; `cx.after(|| ..)`
  runs after the reply); `RATE_LIMIT: u32 = 60` a minute per client, then 429;
  `CORS: &str = "*"`; `TIMEOUT: u32 = 5` → 503; `MIDDLEWARE: &[&str] = &["auth"]`
  runs `pub fn auth(cx: &mut Cx) -> Result` of `src/middleware.rs` first, in order;
  `SIGNED_IN: bool = true` in a `+layout` block: its pages and actions are for
  members (303 to sign in, 401 for JSON); `PRERENDER: bool = true` (`fn entries()`
  for params): `wisp build` renders once, the binary serves the bytes (`cx` in it
  is a build error; `--static` prerenders every page); `SSR: bool = false`: the
  browser draws the page (`wisp build --spa`). `CACHE`, `CACHE_PUBLIC`, `SSR`
  and `PRERENDER` in a `+layout` block are its pages' unless a page sets its own.

## Templates (Rust on the server)

| Syntax | Meaning |
|---|---|
| `{expr}` | escaped Display; an `Option` shows nothing for None |
| `{@html expr}` | raw (trusted only) |
| `attr={expr}` | quoted+escaped; `Option` → left out when None |
| `disabled={bool}` `<a {href}>` `class:on={bool}` | boolean attr, `href={href}`, toggled class |
| `{#if c}…{:else if c}…{:else}…{/if}` | `if let Some(x) = y` works; a bare `{#if x.avatar}` tests `Some`, non-empty or `true` |
| `{#each list as item, i if cond}…{:else}…{/each}` | `if` filters, `{:else}` when empty |
| `{#match e}{:case P}…{/match}` | match |
| `{#await f}…{:then v}…{:catch e}…{/await}` | page only: sent pending, `v`/`e` streamed in later |
| `{@const x = expr}` | let |
| `{#snippet row(a, b)}…{/snippet}` `{@render row(x, 1)}` | local markup fn |
| `{@pager posts}` | Newer/Older links of a `Table::page` |
| `<a href="/blog" active>` | `aria-current="page"` on `/blog` and below (`/` only itself); pages, layouts |
| `{@flash}` | `cx.flash(..)`'s message, once: `<p class="flash" role="status">`; page or layout |
| `<title description="…" image="/og.png">T</title>` | also description, `og:*` and `twitter:card` meta in the head |
| `<head>…</head>` | into the document head; a top-level `<title>` goes there alone; one `<title>` per page: the innermost page or layout with one writes it |
| `<slot />` or `{@render children()}` | layout/component slot |
| `cx` | the request (`&Cx`) in pages, layouts, error pages |

`<style>h1 { color: red }</style>` (top level, no attributes) styles this
file only (`:global(x)` opts out; `<style global>`); it joins app.css.
Accessibility lints warn, never fail (img alt, input label, link and button
name, heading order…); `<!-- wisp-ignore a11y-img-alt -->` silences one.
Images: `<img src="$lib/p.jpg" alt="">` or `src="/x.png"` gets
`width`/`height`, WebP `srcset` (cwebp), lazy; `<img priority>` above the fold;
`data-wisp-raw` opts out. `{#await f}`: `f` is a future, `Send + 'static` (no `cx`
or borrowed locals); the page goes out at once, each answer follows in the same
response; not in layouts, components, `<head>`, attributes, nor with `CACHE`.
Holes can't go in `on*` attrs, tag names, `javascript:` URLs, SVG animation
values or `<meta http-equiv>`; `<script>`/`<style>` bodies have none.
Translations (`src/locales/en.json`, `{t("hi", name = user.name)}`, plurals,
`[[lang=locale]]`, `wisp::alternates(cx)`, `switcher(cx)`, `format_money`):
https://wispweb.dev/docs/design-tooling.

## Actions (form posts)

```rust
#[action]
fn like(id: u64, email: Email, note: Option<String>, agree: bool, tags: Vec<String>) {
    cx.flash("Liked"); // `{@flash}` shows it; cx is added when the body uses it
    redirect("/") // 303; or end in `;` to re-render the page
}
```

- No `->`: returns `Result` (`?`, `return error(..)`, end in `redirect(..)`
  or `;`); `.await` makes it async. Or return `Response`/`Option<Response>`
  to answer instead of the page. `#[action]` is left out for a lone `fn
  default` and each fn the markup posts to (`?/name`).
- `<form action="?/like">` posts (`method="post"` is added); `<form
  method="post">` → `fn default`; `<button action="?/rm&id={x.id}">` outside
  a form is a one-button form.
- `<form fields>` (posts to `fn default`; or `action="?/join" fields`) writes
  a labelled input per param: `Email` → `type=email`, `Password`/`*_password`
  → password, `Image` → file, `bool` → checkbox, numbers → number, text named
  `body message bio comment description notes content` → textarea, else text.
  `fields={post}` starts a struct param's fields from `post` (an edit form).
  `#[validate(one_of = "draft live")]` text is a `<select>`. No button in it:
  one is added (`Send`; `Save` with `{post}`; else the action's name):
  `<form fields />`, `<form fields="Log in" />`.
- Params by name: route param, then form, then query. `T` required (400
  missing; not a `T` → 422 by field), `Option<T>` missing/blank → None,
  `bool` checkbox, `Vec<T>` repeated, `&str`, `Email`, or a `#[model]`
  struct: `fn default(post: Post)` reads its fields by name, blank = missing.
- Uploads: `avatar: Image` (`Option<Image>` may be empty): PNG, JPEG, GIF,
  WebP or AVIF by its bytes (not SVG), else 422; at most 2 MB
  (`wisp::MAX_SIZE`) unless `#[validate(max_size = 5 * MB)]`. The form gets
  `enctype="multipart/form-data"`, the input `accept="image/*"`. Keep it in a
  table field; serve it with `fn get(id: u64) -> Option<Image> {
  USERS.get(id)?.value.avatar }` in `avatars/[id=int]/+server.rs`.
- Rules: `#[validate(len = 1..=100)]` (also `min max min_len max_len email url
  one_of pattern with`) or `return invalid("field", "msg")` → 422, the page
  re-rendered listing every failing field. A `Password` is at least 8 characters
  unless its own `min_len`/`len` says. Inputs get the matching browser checks
  (`required`, `minlength`, `type=email`, `min`/`max`); the server checks all; a
  button with `formaction="?/other"` skips them.
- A refused form's inputs show what was sent, else their own value
  (`value={post.title}`, `<textarea name="body">{post.body}</textarea>`,
  `<select name="kind" value={post.kind}>`), then `<small class="problem">msg</small>`
  (passwords, files: the problem only); ticked boxes and choices are kept;
  `{cx.problem("field")}` puts it elsewhere. Same-origin checked. Works without JS.

## Data and API

- `cx`: `path() param(n) query(n) input(n) form() body() header(n) bearer()
  client_ip() method`, `query_or header_or cookie_or(n, d)`; `locale()
  cookie set_cookie delete_cookie signed_cookie set_signed_cookie`; `sign_in
  sign_out signed_in user login signup`; `flash(msg) flashed() problem(n)`;
  `set(v) get::<T>() take::<T>()`; `fail(status, v) set_status set_header
  cors(o)`; `writes() need_bearer(env) need_signature(env, header)`.
- Errors: `error(404, "msg")`, `redirect("/x")`, `invalid("f", "msg")` return
  `Result`; `opt.or_404()?`, `.or_status(403)?`; `Error::new(s, m)`; any
  `std::error::Error` via `?` → 500.
- `Response::`: `json_of(&v) created(&v) text html empty(s) download(name,
  bytes) file_in(dir, name).await stream ndjson events websocket`
  + `.with_status(s) .with_header(n, v)`. A single-valued header
  (`content-type cache-control location etag`) set again replaces the first.
- `Table<T>`: `add(v)→id get(id) all() find(f) filter(f) update(id, f)
  set(id, v) remove(id) len() page(cx, 10)`; rows are `Row { id, value }` that read as
  the value. `Shared<T>` (`.lock()`), `wisp::provide(v)`/`state::<T>()`,
  `wisp::env("K")`, `spawn`, `every`, `wisp::channel("x")`
  `.send/events()` (SSE)`/websocket()`, `RateLimit::per_minute(n).check(key)?`,
  `#[derive(Cookie)]`. Rows live in `WISP_DATA` log files; `wisp::store(MyDb)`
  in `init` uses any DB; edge: env `WISP_STORE=d1:DB|deno-kv|libsql://…`.
- Saved tables: `#[unique]` on a `#[model]` field, `#[json(default)]`/`#[json(default =
  expr)]`/`#[json(was = "old")]` for old rows, `.migrate(f)`, `.live()` (pages
  naming a live table's static refresh themselves), `set clear by try_add`. A
  field added to a saved type must be `Option`, `Vec` or `bool`.
  Jobs: `wisp::queue(n).push(&j)` + `work(n, f)` + `cron("0 3 * * *", f)` (an
  app with none has no jobs in its wasm), `wisp::cache(k, secs, f)`/`uncache(path)`:
  https://wispweb.dev/docs/data.
- Test: `let mut app = wisp::test::client::<App>(); app.get("/").text()`,
  `app.post_form/post_json/delete`, `.json::<T>()`, `r.location()`,
  `app.upload(url, field, mime, bytes)`, `app.sign_in(id)`,
  `app.modules(&page)`, `app.websocket(url)` (`.send/.recv`). `wisp::test::fresh()` empties every table.
  Browser test: `wisp test --browser`, `let mut b = wisp::browser!(App); b.goto("/");
  b.click("text=Go"); b.text("output")`; also `fill press attr count eval`.
- Env, no code: `WISP_LOG=json` (a line per request, `x-request-id`),
  `METRICS_KEY=k` (`/_wisp/metrics`, Prometheus), `OTEL_EXPORTER_OTLP_ENDPOINT`
  (spans; `wisp::span("x")`, `wisp::traceparent()`), `WISP_HANDLER_TIMEOUT=secs`
  (503), `WISP_HSTS=on`; `/_wisp/health`. Every env var: https://wispweb.dev/docs/env;
  commands and flags: /docs/cli; knobs, Cargo.toml keys, cargo features: /docs/config.

## Components (`src/components/Name.wisp`)

```html
{@props title, count: u32 = 0, featured: bool = false, row: Snippet<&Post, usize>}
<h2>{title}{#if featured} ★{/if}</h2>{@render children()}
```
Use: `<Card title={post.title} count={3} featured>kids</Card>`. Props are
checked at build (no type = `&str`; none = required). No `---` block in
components. `{@element "x-card"}` first also builds it as a custom element
(`/_app/c/el/x-card.js`): `<x-card title="Hi">kids</x-card>` works on any site.
In Rust (a mail body): `Card::html("Hi", 3, false)` is the HTML string.
Plugin crates (`[package.metadata.wisp] use = ["kit"]`) and layers
(`extends = ["../base"]`): https://wispweb.dev/docs/config.

## Endpoints (`+server.rs`)

post/put/patch/delete refuse a request another site sent (`Origin`, else
`Sec-Fetch-Site`; 403, as actions do); `const CORS` (it takes other sites) or
`const CSRF: bool = false;` in the file opts out. GET and curl are unaffected.

Or in the page's block, as `mod server { … }`: the whole route in one file,
built as the three files are (a module of its own: what both halves use goes
in `src/*.rs`). Not with a `+server.rs` beside it.

```html
---
// src/routes/todos/+page.wisp
struct Data {
    count: usize,
}

fn load() -> Data {
    Data { count: TODOS.len() }
}

#[action]
fn add(todo: Todo) {
    TODOS.add(todo);
}

mod server {
    // DELETE /todos/[id]
    fn delete(id: u64) {
        TODOS.remove(id);
    }
}
---

<p>{count} todos</p>
<form action="?/add" fields />
```

A whole JSON API, saved across restarts (`src/routes/api/notes/+server.rs`):

```rust
#[derive(Rest)] // Json + FromJson + Note::table()
#[rest(write = "API_KEY")] // writes need Bearer $API_KEY
struct Note {
    #[validate(len = 1..=200)]
    title: String,
    done: bool,         // left out: false; Vec: []; Option: None
    created_at: String, // set by Wisp (also updated_at)
}
// also before_update, after_*
fn before_create(note: &mut Note) -> Result {
    Ok(())
}
```
→ GET/POST `/api/notes`, GET/PUT/PATCH/DELETE `/api/notes/[id]`; rows are
`{"id":1,…}`; 201, 404, 422 by field. Filters, sorting, pages, ETags, ndjson,
RFC 9457, webhooks, OpenAPI, TypeScript client: https://wispweb.dev/docs/api. A
handler the file writes replaces that one; by hand:

```rust
// GET /api/notes
fn list() -> Vec<Note> {
    db::all()
}
fn post(body: New) -> Response {
    Response::created(&db::add(body))
}
// `id` → /api/notes/[id]
fn get(id: u64) -> Option<Note> {
    db::find(id)
}
```
`fn before(cx) -> Result` in the file runs before each handler.

`body: T` = the JSON body (422 with `errors` by field); other params by name
as for actions. Returns: a `#[model]`/`Json` value → 200; nothing → 204;
`Response`; `Option<T>` (None → 404). Errors are JSON
`{"status","code","error","errors"}` (`Error::new(409, "x").with_code("taken")`)
for endpoints, `/api`, apps with no page, and clients preferring JSON to HTML;
else the `+error.wisp`, else a default page. `const BODY_LIMIT: usize = 20 * wisp::MB;`.
`Response::websocket(|ws| async move { while let Some(m) = ws.recv().await {
ws.send(m).await?; } Ok(()) })`: binary, node, bun, deno, cloudflare, pages
(the rest answer 501; https://wispweb.dev/docs/deploy).

## Members

```rust
// src/db.rs: a `Password` field and `email` (or `name`) make a #[model] an Account
#[model]
struct User {
    #[unique]
    email: Email,
    password: Password,
}
pub static USERS: Table<User> = Table::saved();
```
```html
---
#[action] // sign up
fn signup(email: Email, password: Password) {
    cx.signup(User { email, password }).await?; // hashes it; 422 if taken
    redirect("/me")
}
#[action] // log in
fn login(email: Email, password: String) {
    cx.login(email, password).await?; // 422 for either wrong, equally slow
    redirect("/me")
}
let me = cx.user()?; // Row<User>, or 303 to /login
---

<h1>{me.email}</h1>
```
Both sign in. `user`, `login` and `signup` take the table for you: the only
`Table` of a model with a `Password` field in `src/db.rs`, else the one
`wisp::users(&db::USERS)` names in `init`; or pass `&USERS` first. A
`Password` typed is at least 8 characters (`#[validate(min_len = 12)]` to
change it); log in with a `String`. `cx.signed_in()?` is the id (`.ok()`: no
redirect; JSON clients get 401), `cx.sign_out()`, `wisp::sign_out_everywhere(id)?`,
`wisp::sign_in_page("/enter")`. `wisp::login`/`signup` are these without a Cx.
Hashes: PBKDF2-SHA256, ~0.2 s off the worker (`RateLimit` sign-ins); a
`Password` is `Plain` as typed and `Hashed` once a table or `signup` hashes
it; `null` in any JSON out. `cx.need(&USERS, |u| u.admin)?` is the Row, 403
if not allowed. `token`/`untoken` links, `totp`, `oauth`: https://wispweb.dev/docs/auth.

## Browser code (JavaScript, same file)

`{…}` is Rust on the server; quoted directive values and `{:…}` are JS.

`<button on:click="count++">{:count}</button>` with `<script>let count = 0</script>`
(top-level lets are state). A page's Rust names are browser values by name (`items`, `data.items`).
`bind:value="q"` with no `let q` declares it, and so does a handler that
toggles (`open = !open`: false) or counts (`n++`: 0) a name nothing declares,
so a live search needs no script: `<input bind:value="q">` `{:#each items as i
if matches(i.name, q)}…{/each}`. Directives `on:click` (`.prevent .once .debounce.300ms`…),
`bind:value|checked|this`, `:attr="js"`, `:text`, `class:x="js"`,
`transition:fade`, `use:action`; client blocks `{:#if}` `{:#each}`, in them `{:@const x = e}`
and `{:@html h}`; `{:@render row(x)}` draws a `{#snippet}` or a snippet prop; runes
`$state $derived $effect $props`; helpers `onMount listen goto invalidate matches
tick`. Values sent to JS must be `#[model]` or `#[derive(Json)]`.
`#[remote] fn user(id: u64) -> Result<User>` (page block or `src/*.rs`) is
`await user(5)` in any script (`src/lib`: `import { user } from 'wisp:remote'`);
`#[remote(get)]` a GET; errors reject with `status`, `message`, `errors`.
`<script lang="ts">`, `src/lib/*.ts` (types stripped; `wisp check --types`).
`env.PUBLIC_X` is filled at build. `npm`: `wisp add pkg`; `<Island
of="react:react-switch" client:visible props={:{...}} />` (`react|preact|vue|svelte`).
`onclick="…"` doesn't run (CSP): use `on:click`.
Router (`import {...} from 'wisp'`: `beforeNavigate afterNavigate preloadData
invalidateAll pushState`), stores, PWA (`src/manifest.json`, `"offline": true`),
`<body data-wisp-revalidate>`, `<form data-wisp-queue>`, `data-wisp-noscroll|
keepfocus|replacestate|notransition|keep` links, `<script type="wisp/idle">`,
`<meta name="wisp-vitals">`, custom bundles: https://wispweb.dev/docs/client.

## hooks.rs and config

```rust
async fn init() -> Result {
    wisp::provide(Db::connect(&wisp::env("DB_URL").or_status(500)?).await?);
    Ok(())
}
fn before(cx: &mut Cx) -> Result {
    cx.cors("*")?; // a preflight is the Err that `?` returns
    Ok(())
}
fn after(cx: &mut Cx, reply: &mut Reply) {} // sync, every reply: headers, logs
fn report(cx: &mut Cx, err: &Error) {} // sync, every 5xx: Sentry and the like (handleError)
// sync, before routing: return a part of `path`
fn reroute(path: &str) -> &str {
    path
}
```
`after`/`report`/`reroute` cost nothing in an app that has none. Keep `before`
sync: `async fn before` takes the no-wait fast path off every route. No other
`pub fn` here. `#[derive(Config)] struct Conf { api_key: String, port: Option<u16> }`
(any `src/*.rs`) reads `API_KEY`, `PORT` (env or `.env`) before `init`; one
wrong stops the start, naming it; `Conf::get().api_key` anywhere.
`routes::blog_slug(slug)` (in every route file; `routes::home()`) is the path
`/blog/<slug>`: a link that names a route gone does not compile. Pages get a
`content-security-policy` (`wisp::csp("img-src 'self' https://x")` in `init`
replaces a directive; `wisp::csp_off()`). Cargo.toml `[package.metadata.wisp]`:
`redirects = ["/old/[id] /new/[id] 301"]`, `rewrites = ["/g/[...p] /docs/[...p]"]`,
`headers = ["/api/[...p] x-a: b"]`, `base = "/app"` (or `WISP_BASE`), `i18n`,
`auto`, `use`, `extends`; checked at build, none declared costs nothing.
`wisp::trailing_slash(Always)` in `init` (`Never` default, `Ignore`);
`wisp::on_fetch(|req| ..)` wraps every `wisp::fetch`: https://wispweb.dev/docs/config.

## Markdown pages

`+page.md` with `---` front matter (`title`, `layout: Post` a component the
page is the children of, any field `date: 2026-10-01`); text may use `<Card>`
between blank lines. Built at build time; fenced code is highlighted (`hl-k
hl-s hl-c hl-n hl-t hl-a`); headings get ids. `noindex: true` leaves the
sitemap. `/feed.xml` is an Atom feed of pages with a `date`. Index: `{#each
wisp::pages("blog") as p}<a href={p.path}>{p.title}</a>{/each}` (newest first).
`{@html wisp::og(title, desc, image)}` in a head: Open Graph tags (image
`"auto"`: an SVG `wisp build` writes). More: https://wispweb.dev/docs/design.

## Gotchas

- `Err(error(..))` is wrong: `error()` already returns the `Result`.
- `+page.wisp` needs the `+`. Block statements and `fn load` are exclusive;
  a block's last statement ends with `;`.
- Don't hold `Shared::lock()` or other guards across `.await`; blocking work
  → `tokio::task::spawn_blocking` (a thread per core).
- `{#each x as y}` borrows a field path; `.iter()` other expressions.
- In `+server.rs`, a param named `id` (no `[id]` folder) serves `/[id]`: use
  `list` for the folder's GET. `#[validate]` on params is for actions.
- HTTP/2 in process is opt-in: `wisp = { .., features = ["h2"] }` (h2c, no
  TLS); app code is the same.

## Commands

`wisp new app [--template demo|minimal|api]` · `wisp dev` (hot reload keeps
`$state`; every open tab updates after each rebuild; the error dialog opens
`file:line` in the editor; `Server-Timing` on each dev response; `Alt+Shift+W`
devtools) · `wisp test [--browser]` · `wisp check [--types] [--rust]
[--explain-imports]` · `wisp fmt [--check]` · `wisp build` (`--static`,
`--spa`, `--docker`, `--target cloudflare|pages|deno|vercel|netlify|node|bun|lambda|native`,
`--edge`, `--client ts`, `--sourcemap`, `--analyze`) · `wisp deploy init
<host>` · `wisp openapi [-o openapi.json] [--check]` · `wisp service
install|uninstall|start|stop|status` · `wisp routes` · `wisp add
page|form|layout|server|rest|api /path`, `wisp add crud /posts` (list, new,
edit pages; `Post` and `POSTS` into `src/db.rs`), `wisp add component Card`,
`wisp add|remove pkg` (`wisp add` alone lists the recipes in `add/`) · `wisp ui
add|list button dialog` (accessible components into `src/components`) · `wisp
lsp` · `wisp update-docs` · `wisp mcp` (`claude mcp add wisp -- wisp mcp`) ·
`wisp --version`. A CLI older than the app's `wisp` crate warns first
(`cargo install wisp-web --force`; `WISP_NO_UPDATE_CHECK=1`).
Docs: https://wispweb.dev/docs (client, api, data, auth, serve, deploy,
embed, design, tokens), or llms-full.txt (this file and the site's pages).
