# Wisp design

Wisp is a fast, fun web framework for Rust. Server-rendered HTML, file-based
routes, `.wisp` templates, form actions that update the page without a client
framework, and a single binary to deploy.

This document is the contract for v0. When code and doc disagree, fix one of them.

## Principles

1. **Fast by construction.** Templates compile to straight-line `push_str` calls.
   Routes compile to one `match`. Buffers are reused per connection. No boxing,
   no dynamic dispatch, no allocation on the hot path after warm-up.
2. **Minimal dependencies.** The runtime depends on `tokio` and `httparse`. The
   build crate and CLI depend on nothing but std. Every new dependency needs a
   written reason in this file.
3. **Boring code.** Plain functions and plain data. Abstractions only where they
   remove more code than they add. Invariants are asserted, not assumed.
4. **Fast dev loop.** Editing markup never waits for `cargo`. Editing Rust
   rebuilds only the app crate.
5. **Works without JavaScript.** Forms are real forms and links are real links.
   `wisp.js` enhances them; it is never required.

## Dependency budget

| Crate      | Used by       | Why it exists                                                      |
|------------|---------------|--------------------------------------------------------------------|
| tokio      | wisp          | Async runtime; the entire DB/client ecosystem assumes it.         |
| httparse   | wisp          | Zero-dep, fuzzed HTTP/1.x header parser (the one hyper uses).     |

Deliberately *not* used: hyper, axum, tower, serde, a TOML parser, `notify`,
a proc-macro stack (`syn`/`quote`). Things we write ourselves instead: the
HTTP/1.1 connection loop, URL/form decoding, HTML escaping, the HTTP date, the
template compiler, a polling file watcher, the dev proxy of events.

Future, feature-gated only: an AEAD crate for signed component state (we do not
write our own crypto), optional `serde` integration.

## Workspace

```
crates/wisp        runtime: HTTP server, Cx, escaping, assets, dev hooks
crates/wisp-build  compiler: route scan, .wisp parser, codegen (used from build.rs)
crates/wisp-macros #[action] marker attribute (identity proc macro, no deps)
crates/wisp-cli    `wisp new | dev | build`
examples/demo      the demo app, which is also `wisp new`'s demo template
bench/             load generator + ASP.NET Core baseline
```

## An app

```
my-app/
  Cargo.toml          deps: wisp; build-deps: wisp-build
  build.rs            fn main() { wisp_build::run() }
  src/main.rs         wisp::app!(); fn main() { wisp::run::<App>() }
  src/app.html        document shell with %wisp.head% and %wisp.body%
  src/app.css         optional; Tailwind if it contains @import "tailwindcss"
  src/routes/...      pages
  static/...          served as-is at /
```

### Routes

Directory names are URL segments. Files that start with `+` are route files.

| File           | Meaning                                                           |
|----------------|-------------------------------------------------------------------|
| `+page.wisp`   | Markup for the page at this path.                                 |
| `+page.rs`     | `load` for the page and its `#[action]` functions.                |
| `+layout.wisp` | Wraps this page and every page below it. `{@render children()}`.  |
| `+layout.rs`   | `load` for the layout.                                            |
| `+error.wisp`  | Rendered for errors below this directory. Gets `status`, `message`. |
| `+server.rs`   | `pub async fn get/post/put/patch/delete` endpoints.               |

Segment syntax: `blog` (static), `[slug]` (param), `[[lang]]` (optional),
`[...rest]` (rest, may be empty), `(group)` (not part of the URL).

Priority when several routes match: static segment > param > optional > rest,
compared left to right. Two routes that resolve to the same pattern are a build
error. `/about/` redirects (308) to `/about`.

### Page logic

```rust
// src/routes/blog/[slug]/+page.rs
use wisp::prelude::*;

pub struct Data { pub post: Post }

pub async fn load(cx: &mut Cx) -> Result<Data> {
    let post = db::post(cx.param("slug")).await.or_404()?;
    Ok(Data { post })
}

#[action]
pub async fn like(cx: &mut Cx) -> Result<()> {
    let id: i64 = cx.form().required("id")?.parse()?;
    db::like(id).await?;
    Ok(())
}
```

- `load` is found by name, actions by the `#[action]` marker. Nothing else in the
  file is reachable from HTTP. This is deliberate: a helper `pub async fn` must
  never become an endpoint by accident.
- `load` returns `Result<Data>`; `Data` must be a public type in `+page.rs`
  (defined or re-exported) with public fields, because the template reads it.
- Errors: `?` on any `std::error::Error` gives a 500 (details only in dev).
  `error(404, "…")` and `redirect(303, "/…")` construct control-flow errors.
  `Option::or_404()` is the common shortcut.

### Templates

```html
<wisp:head><title>{data.post.title}</title></wisp:head>

<h1 class="text-3xl font-bold">{data.post.title}</h1>

{#if data.posts.is_empty()}
  <p>No posts.</p>
{:else if data.posts.len() == 1}
  <p>One post.</p>
{:else}
  {#each data.posts as post, i}
    <a href="/blog/{post.slug}" class={post.class()}>{i}: {post.title}</a>
  {:else}
    <p>Unreachable, but {#each}…{:else} renders when the list is empty.</p>
  {/each}
{/if}

{#match data.status}
  {:case Status::Draft} <span>draft</span>
  {:case Status::Live(date)} <time>{date}</time>
{/match}

{@const total = data.posts.len()}
{@html data.trusted_svg}
```

| Syntax                         | Compiles to                                        |
|--------------------------------|----------------------------------------------------|
| `{expr}`                       | escaped `Display` of `expr`                        |
| `attr={expr}`                  | `attr="…"`, quotes added, value escaped            |
| `{@html expr}`                 | unescaped `Display` (you promise it is safe)       |
| `{@const x = expr}`            | `let x = expr;`                                    |
| `{#if c}…{:else if c}…{:else}…{/if}` | `if`/`else`; `if let` works as in Rust        |
| `{#each e as pat[, i]}…{:else}…{/each}` | `for`; a plain place like `data.posts` is borrowed |
| `{#match e}{:case pat}…{/match}` | `match`; a plain place is borrowed               |
| `{@render children()}`         | layout slot                                        |
| `<wisp:head>…</wisp:head>`     | appended to the document head                      |

Expressions are Rust, passed to `rustc` verbatim, so type errors are real type
errors. Inside `<script>`, `<style>` and HTML comments there are no holes, so
CSS and JS braces need no escaping. Comments are stripped. Whitespace runs that
contain a newline collapse to one newline, except in `<pre>`/`<textarea>`. A
block tag (`{#…}`, `{:…}`, `{/…}`, `{@const}`) alone on its line leaves no line
behind, so loops don't print blank lines between items.

Escaping covers `& < > " '`, which is safe in text and in quoted attributes.
Unquoted `attr={…}` is always quoted by the compiler. There is no way to put an
expression inside `<script>`; pass data through `data-*` attributes instead.

Components (`<Card title={x}>…</Card>` from `src/lib/*.wisp` with
`{@props title: &str}`) and snippets are v0.2.

### Actions and `wisp.js`

`<form method="post" action="?/like">` posts to the `like` action; a form with no
`action` posts to the action named `default`. Flow:

1. Same-origin check: if `Origin` is present it must match `Host` (403 otherwise).
2. The action runs. `Err(redirect)` → 303. Other errors → error page.
3. On success the page's `load` runs and the page is rendered as a normal
   response. Without JS the browser just shows it.
4. `wisp.js` intercepts the submit, sends it with `fetch`, then morphs `<body>`
   in place (keyed by `id`), so focus, scroll and unrelated inputs survive.
   Redirects followed by `fetch` update the URL with `history.pushState`.
   The submit button is disabled while the request is out and re-enabled
   before the morph, so the new page decides its final state.

State that belongs to one visitor goes in a cookie: `cx.set_cookie(name, value)`
sets it site-wide for 400 days, `HttpOnly`, `SameSite=Lax` (an empty value
deletes it), and `cx.cookie(name)` reads it back within the same request, so
the `load` that runs after an action sees what the action stored. Values are
not signed; anything a visitor must not forge waits for signed state.

`wisp.js` target: under 4 KB unminified. v0.2 adds a small fixed set of
declarative behaviors (toggle, tabs, filter, hotkey, optimistic) and link
boosting. Dispatching `wisp:refresh` on the document morphs the current URL's
page in again. Dev code lives in `wisp-dev.js`, which only debug builds serve
and link, so none of it ships in production pages.

### Built-in UI

Wisp draws two screens of its own, both in the same restrained dark style: a
near-black page (`#141414`), panels one step lighter (`#1f1f1f`) with
one-pixel hairlines, one tight type scale, and Wisp violet (`#896ce0`) only for
what is interactive. Text on a violet fill is dark, which reads at 4.6:1.

- **The default error page**, for apps without a `+error.wisp`. It says what
  happened, the reason in the server's words, and the status with the request
  (`404 · GET /nope`) as a reference line. Its styles come with it and are
  scoped to `.wisp-error`, since the app's own CSS may not exist yet.
- **The build error dialog** in dev: the compiler's output in a code block
  with a Copy control, over the page that is still running. It lives in a
  shadow root hung off `<html>`, so neither the app's CSS nor a page morph
  can touch it, and it closes by itself when the next build succeeds.

## Runtime

- `wisp::run::<App>()` serves on `$HOST:$PORT` (default 3000), thread per
  core: one single-threaded tokio runtime per CPU (`$WISP_THREADS`), each with
  its own I/O driver, and the main thread accepting connections and handing
  them out in turn. A connection lives on one thread, so the request path never
  wakes another thread. A multi-thread tokio runtime funnels every socket event
  through one driver; measured, it left more than half the cores idle at under
  half the throughput (bench/README.md). The tradeoff: no work stealing, so a
  handler that blocks its thread stalls that thread's connections.
- `wisp::serve::<App>(addr)` is the async form, for apps that must own their
  runtime.
- One task per connection. `Cx` owns the connection's read buffer; the task also
  owns a write buffer and an `Out { head, body }` pair of `String`s, all reused
  across requests.
- HTTP/1.1 with keep-alive and pipelining: every complete request in the read
  buffer is answered into the write buffer, then one `write_all`.
- Requests are parsed in place (`httparse`) and recorded in `Cx` as byte spans
  into its buffer, so `Cx` has no lifetime and handlers take `&mut Cx`.
- Limits: 16 KB of headers, 1 MB body, 64 headers, 10 s to receive a request,
  60 s keep-alive idle. `Transfer-Encoding` on requests → 501 (put a proxy in
  front for chunked uploads); both `Content-Length` and `Transfer-Encoding` → 400.
- `Date` is cached per thread and reformatted once per second.
- A panic in a handler becomes a 500 for that request; the connection survives.
- HTTP/2, TLS and compression belong to the reverse proxy / CDN (Caddy, nginx,
  Cloudflare). This keeps the binary small and the hot path simple.

Generated code implements one trait:

```rust
pub trait App: 'static {
    fn handle(route: usize, cx: &mut Cx, out: &mut Out) -> impl Future<Output = Result<()>> + Send;
    // + static tables: shell, assets, templates (dev)
}
```

`handle` is a single `match` over the route id, so the whole server is
monomorphized with the app. There are no handler trait objects anywhere.

## Build

`wisp_build::run()` (in the app's `build.rs`):

1. Walks `src/routes`, builds the route table, sorts by priority, rejects conflicts.
2. Parses every `.wisp` file into a node list. Errors are `file:line:col: msg`.
3. Scans `+page.rs`/`+layout.rs`/`+server.rs` with a tiny Rust lexer for `fn load`,
   `#[action] … fn name` and HTTP-method functions.
4. Writes `$OUT_DIR/wisp.rs`: `#[path]` modules for the user's files (so
   rust-analyzer sees them as normal modules), one render function per template,
   the router `match`, `handle`, and asset tables.

Release builds embed `static/` and the built CSS into the binary with a content
hash, served with `Cache-Control: immutable` under `?v=hash` URLs.

## `wisp new`

`wisp new [name]` asks where the app goes, which template (Demo: a home page
with a counter, an about page and Wisple, a word game built on form actions;
Minimal: one page, a layout and an error page), whether to add Tailwind,
whether to create a git repository (yes unless the app lands inside one, like
`cargo new`), and whether to download and compile dependencies now. Every
question has a flag (`--template`, `--[no-]tailwind`, `--[no-]git`,
`--[no-]install`); `--yes`, or no terminal to ask on, takes the defaults.
The prompts are plain lines on std, not a cursor-driven menu.

Until Wisp is on crates.io, apps depend on it by path when `wisp` was built
from a clone (`cargo install --path crates/wisp-cli`), so changes to Wisp
reach them at once, and on https://github.com/wyziedevs/wisp when it was
installed with `cargo install --git`.

The demo template is `examples/demo` itself, read with `include_str!`, so the
two cannot drift. With Tailwind, the template's styles go in `@layer base`
after the import, so utility classes still win over them.

## Dev loop

`wisp dev` is one std-only process:

- Polls `src/`, `static/`, `Cargo.toml` and `build.rs` mtimes every 50 ms (no
  `notify`).
- Runs Tailwind standalone `--watch` into `.wisp/app.css` if `src/app.css`
  imports Tailwind; otherwise `src/app.css` is served as written.
- Builds with `cargo build`, copies the exe to `.wisp/run/` (so the next build can
  overwrite the original while the old server keeps serving), then restarts it.
  The app is ready when it prints its `listening on` line; the CLI reads the
  app's stdout rather than polling the port (a refused connect takes 2 s to
  fail on Windows).
- Serves a Server-Sent Events stream on its own port. Browsers stay connected
  across app restarts and are told to morph/reload once the new app is ready.

Template hot swap: in debug builds every static HTML chunk of every template is
read through a table (`wisp::dev::chunk`) instead of being a literal. On a
`.wisp` save the CLI re-parses the file; if its *shape* (holes, blocks,
expressions — everything except static text) is unchanged, it POSTs the new
chunks to the app (`/_wisp/dev/swap`, loopback only) and the browser morphs.
No compile. If the shape changed, it is a normal rebuild. Release builds have
no table: chunks are literals.

Rust build tuning shipped in the app template: `debug = "line-tables-only"`,
dependencies at `opt-level = 1`.

Targets: markup edit → visible < 100 ms. Rust edit → visible ≤ 3 s (small app).
Measured on the demo (Windows, Ryzen 7800X3D): a text edit is swapped in under
1 ms and served about 85 ms after the save (mostly the 50 ms poll); an
expression or `.rs` edit rebuilds and restarts in 0.3 s.

## Security

- Escaping by default; `{@html}` is the only raw output.
- Actions are opt-in (`#[action]`), same-origin checked, and take only form
  fields — no client-supplied type names or serialized state (Livewire
  CVE-2025-54068 class).
- Future signed component state: AEAD, expiry, session binding, closed schema.
- Dev endpoints exist only in debug builds and only answer loopback peers.
- Request size and time limits as above; no request smuggling surface
  (no chunked requests, CL+TE rejected).

## v0 non-goals

ORM, auth, background jobs, i18n, a client-side router, WebSockets, HTTP/2 in
process, Windows services. Each is either a library users pick or a later version.

## Milestones

1. **Core** – routes, layouts, templates, load, actions, errors, static files,
   `wisp.js` morph, `wisp dev` with hot swap.  ← current
2. **Measure** – dev-loop timings; req/s and latency vs ASP.NET Core Minimal
   APIs on the same machine.
3. **v0.2** – components, snippets, behaviors, link boosting, sessions,
   param matchers, signed state, docs site built with Wisp.
