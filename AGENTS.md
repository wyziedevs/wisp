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
src/app.css | app.scss      served at /_app/app.css (Tailwind if it imports it; Sass, no Node)
postcss.config.*            PostCSS after either (needs Node + postcss-cli)
package.json                npm packages for browser code: `wisp add canvas-confetti`
.env                        X=…: `wisp::env("X")`, `env.PUBLIC_X` in browser code
src/hooks.rs                fn init() once; fn before(cx) every request
src/db.rs                   models and tables; its `pub` items are in every route file
src/NAME.rs                 any module, no `mod` line: `NAME::f()` everywhere
src/remote.rs               #[remote] fns browser code calls (or in a page's block)
src/components/Card.wisp    <Card title={x}>…</Card>
src/lib/*.js (or .ts)       browser modules, `import … from '$lib/x.js'`
src/params/word.rs          fn matches(s: &str) -> bool, for [x=word]
src/locales/en.json         messages, fr.json etc.: {t("key")}
src/manifest.json           web app manifest: {"name": "Notes", "offline": true}
src/service-worker.js       registered for you: import { build, files, version } from 'wisp/sw'
src/routes/…/+page.wisp     page: optional `---` Rust block, then markup
src/routes/…/+layout.wisp   wraps pages below; must <slot /> (or {@render children()})
src/routes/…/+error.wisp    error page; has `status`, `message`, `cx`
src/routes/…/+server.rs     endpoints: fn get/post/put/patch/delete/list
src/routes/…/+page.md       Markdown page (`blog/x.md` = /blog/x)
src/routes/…/+page.js       optional browser `load({data,url,params,fetch})` (or .ts)
static/…                    served at /
```

Folders: `blog` static, `[slug]` param, `[[lang]]` optional, `[...rest]`
rest, `[id=int]` digits (u64), `[x=word]` custom matcher, `[[lang=locale]]`
one of `src/locales`, `(group)` not in URL. `+page.rs` (`struct Data` + `fn load(..) -> Data`, which the markup reads
by name) and `+layout.rs` work instead of a block.
`/sitemap.xml` (pages without params, or with `entries()`; not `(private)`
groups or `noindex` pages; host from env `SITE_URL`, else the request) and
`/robots.txt` are made; a route or `static/` file of that name wins.

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
  action remote error invalid model redirect Always Never Ignore` and `src/db.rs`'s `pub` items (local
  names win). `Result` alone = `Result<()>`.
- `const CACHE: u32 = 60;` (page or `+server.rs`) keeps a GET's answer 60 s
  per worker (ETag, 304), but never for a request with a cookie or
  `authorization` (`CACHE_PUBLIC`: all), nor one that sets a cookie; not in dev.
- `const PRERENDER: bool = true;` (and `fn entries()` with params): `wisp
  build` renders the page once and the binary serves those bytes (ETag,
  304). `cx` in it is a build error. `--static` prerenders every page.
- `const SSR: bool = false;`: the browser draws the page; the markup must be
  browser code (`{:x}`, `{:#each}`). `wisp build --spa` = `--static` + an
  `index.html` fallback for static hosts.
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

`<style>h1 { color: red }</style>` (top level, no attributes) styles this
file only (`:global(x)` opts out; `<style global>`); it joins app.css.
Accessibility lints warn, never fail (img alt, label control, `<a href>`,
heading order, button text...); `<!-- wisp-ignore a11y-img-alt -->` silences
one. Images: `<img src="$lib/p.jpg" alt="">` or `src="/x.png"` gets
`width`/`height`; `wisp build` adds WebP `srcset` (cwebp, cached), lazy.
`data-wisp-raw` opts out. Translations: `src/locales/en.json` (`{"hi":
"Hello, {name}!", "n": "{count, plural, =0 {None} one {# item} other {#
items}}"}`), `{t("hi", name = user.name)}`, `t('n', c)` in scripts; keys
checked across locales at build. Locale: `[[lang=locale]]`, cookie `lang`,
`Accept-Language`, first; `cx.locale()`, `wisp::locales()`,
`wisp::localize(cx.path(), "fr")`, `wisp::default_locale("fr")?`.

Holes can't go in `on*` attrs, tag names, `javascript:` URLs, SVG animation
values or `<meta http-equiv>`; `<script>`/`<style>` bodies have none.

## Components (`src/components/Name.wisp`)

```html
{@props title, count: u32 = 0, featured: bool = false, row: Snippet<&Post, usize>}
<h2>{title}{#if featured} ★{/if}</h2>{@render children()}
```
Use: `<Card title={post.title} count={3} featured>kids</Card>`. Props are
checked at build (no type = `&str`; none = required). No `---` block in
components. `{@element "x-card"}` first also builds it as a custom element
(`/_app/c/el/x-card.js`): `<x-card title="Hi">kids</x-card>` works on any site.
In Rust (a mail body): `Card::html("Hi", 3, false)` is the HTML string; every prop is
an argument, no children. Plugin crates: `[package.metadata.wisp] use = ["kit"]` in
Cargo.toml copies the dependency's `wisp/routes` and `wisp/components` into the app at
build (to `src/routes/(kit)/`, `src/components/kit/`, git-ignored); the app's own component of
the same name wins. A `path` dependency or one in the registry; no git dependency.

## Markdown pages

`+page.md` with `---` front matter (`title`, `layout: Post` a component the
page is the children of, any field `date: 2026-10-01`); text may use
`<Card>` between blank lines. Built at build time; fenced code is
highlighted (`hl-k hl-s hl-c hl-n hl-t hl-a`; color them). `noindex: true`
leaves the sitemap. Index: `{#each wisp::pages("blog") as p}<a
href={p.path}>{p.title}</a>{/each}` (newest `date` first).

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
matches`. Values sent to JS must be `#[model]` or `#[derive(Json)]`.
`pushState('?tab=2', {tab: 2})`: shallow routing, `page.value.state`; changed
fields are restored with history. `import('$lib/x.js')` loads on demand.
`<script lang="ts">`, `src/lib/*.ts`, `+page.ts` (types stripped; `wisp check
--types`). `env.PUBLIC_X` is filled at build. Dev source maps; `--sourcemap`.
`npm`: `wisp add pkg`; `<Island of="react:react-switch" client:visible
props={:{...}} />` (`react|preact|vue|svelte`); web components just work.
`#[remote] fn user(id: u64) -> Result<User>` (page block or `src/*.rs`) is
`await user(5)` in any script (`src/lib`: `import { user } from
'wisp:remote'`): POST to `/_app/r/<hash>`, `#[remote(get)]` a GET; errors
reject with `status`, `message`, `errors`. PWA: `src/manifest.json` (or
`wisp::app_manifest(json)?`) is `/manifest.webmanifest`, icons from
`static/icon.png`; `"offline": true` adds a service worker.
Stores, islands, the rest: docs/client.md.

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
files; `wisp::store(MyDb)` in `init` uses any DB; edge: env
`WISP_STORE=d1:DB|deno-kv|libsql://…`.

## Members

```rust
// src/db.rs: `hash` and `email` (or `name`) make a #[model] an Account
#[model]
pub struct User { email: Email, hash: String }
pub static USERS: Table<User> = Table::saved();
```
```html
---
#[action]                                       // sign up
fn signup(email: Email, #[validate(min_len = 8)] password: String) {
    cx.signup(&USERS, User { email, hash: password }).await?;  // hashes it; 422 if taken
    redirect("/me")
}
#[action]                                       // log in
fn login(email: Email, password: String) {
    cx.login(&USERS, &email, &password).await?; // 422 for either wrong, equally slow
    redirect("/me")
}
let me = cx.user(&USERS)?;                      // Row<User>, or 303 to /login
---
<h1>{me.email}</h1>
```
Both sign in. `wisp::users(&db::USERS)` in `init` makes it `cx.user()`.
`cx.signed_in()?` is the id (`.ok()`: no redirect; JSON clients get 401),
`cx.sign_out()`, `wisp::sign_out_everywhere(id)?`, `wisp::sign_in_page("/enter")`.
`wisp::login`/`signup` are these without a Cx. Hashes: PBKDF2-SHA256, ~0.2 s
off the worker (`RateLimit` sign-ins); by hand `wisp::password::{hash, check}`.
`cx.need(&USERS, |u| u.admin)?` is the Row, 403 if not allowed. More:
`docs/auth.md` (`token`/`untoken` links, `totp`, `oauth`, `mail`,
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
Pages get a `content-security-policy` (`wisp::csp("img-src 'self' https://x")`
in `init` replaces a directive; `wisp::csp_off()`); `onclick="…"` doesn't run:
use `on:click`. `wisp::trailing_slash(Always)` in `init`: pages are `/about/`
(`/about` gets a 308; `Never`, the default; `Ignore` both; sitemap follows).
No other `pub fn` here. Keep `before` sync: `async fn before` takes the
no-wait fast path off every route.

## API

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
  `app.post_form/post_json/delete`, `.json::<T>()`, `r.location()`,
  `app.upload(url, field, mime, bytes)`, `app.sign_in(id)`,
  `app.modules(&page)`, `app.websocket(url)` (`.send/.recv`). `wisp::test::fresh()` empties every table.
  Browser test: `wisp test --browser`, `let mut b = wisp::browser!(App); b.goto("/");
  b.click("text=Go"); b.text("output")`; also `fill press attr count eval`.
- Env, no code: `WISP_LOG=json` (a line per request, `x-request-id`),
  `METRICS_KEY=k` (`/_wisp/metrics`, Prometheus), `OTEL_EXPORTER_OTLP_ENDPOINT`
  (spans; `wisp::span("x")`, `wisp::traceparent()`).

- Serve extras (docs/serve.md): embedded files gzip + `Range`; pages get
  `nosniff` and `referrer-policy` (`WISP_HSTS=on`, `WISP_SECURE_HEADERS=off`);
  `/_wisp/health`; `WISP_HANDLER_TIMEOUT=secs` â†’ 503.

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

`wisp new app` Â· `wisp dev` (hot reload keeps `$state`; `Alt+Shift+W`
devtools; `/_wisp/components` workshop of `*.stories.wisp`) Â· `wisp test
[--browser]` Â· `wisp check [--types]` Â· `wisp fmt [--check]` Â· `wisp build`
(`--static`, `--spa`, `--docker`, `--target
cloudflare|deno|vercel|netlify|node|bun|lambda|native`, `--client ts`,
`--sourcemap`) Â· `wisp deploy init <host>` (a GitHub Actions workflow; or
`fly|render|railway`: that host's config) Â· `wisp routes` Â· `wisp new-route
/path page|server|rest` Â· `wisp add|remove pkg` Â· `wisp ui
add button dialog` (accessible components into `src/components`) Â· `wisp lsp`
Â· `wisp update-docs` Â· `wisp mcp` (`claude mcp add wisp -- wisp mcp`). Docs:
README.md, docs/design.md, client.md, api.md, deploy.md, embed.md, tokens.md,
or llms-full.txt (this file and every doc).
