# Wisp design

Wisp is a fast, fun web framework for Rust. Server-rendered HTML, file-based
routes, `.wisp` templates, form actions that update the page without a client
framework, and a single binary to deploy.

This document is the contract for v0. When code and doc disagree, fix one of them.

## Principles

1. **Ultra fast, then cheap, then durable, then flexible; developer happiness
   last.** Cheap means app code in as few tokens as possible: AI writes most
   code now, so a developer picks the framework whose apps run fastest, cost
   the fewest tokens to write, keep working and bend furthest. Durable means
   every fast path is proven at startup and falls back, and nothing after
   startup takes the process down. Every feature is judged
   first by what it costs the app's code: a convention beats a line of
   setup, one file beats two, and a name the build can infer is not written.
   [tokens.md](tokens.md) measures it against other frameworks, and
   [AGENTS.md](../AGENTS.md) is the whole language in one short page.
2. **Fast by construction.** Templates compile to straight-line `push_str` calls.
   Routes compile to one `match`. Buffers are reused per connection. No boxing,
   no dynamic dispatch, no allocation on the hot path after warm-up.
3. **Minimal dependencies.** The runtime depends on `tokio` and `httparse`. The
   build crate and CLI depend on nothing but std (and `pulldown-cmark`, for
   Markdown pages at build time). Every new dependency needs a
   written reason in this file.
4. **Boring code.** Plain functions and plain data. Abstractions only where they
   remove more code than they add. Invariants are asserted, not assumed.
5. **Safe.** No `unsafe` anywhere in Wisp or in the code it generates; the
   workspace lint is `forbid`. (The benchmark runner, which pins processes to
   CPUs through the OS, is the one crate that sets its own.)
6. **Mistakes fail early, in the user's own file.** Whatever the build can
   check, it checks, and says where and what to do: a private `load`, an
   `#[action]` in the wrong place, `page.wisp` without its `+`, a block that
   leaves a tag open in one branch. rustc should only ever point at code the
   user wrote.
7. **Fast dev loop.** Editing markup never waits for `cargo`. Editing Rust
   rebuilds only the app crate.
8. **Works without JavaScript.** Forms are real forms and links are real links.
   `wisp.js` enhances them; it is never required.

## Dependency budget

| Crate       | Used by        | Why it exists                                                      |
|-------------|----------------|--------------------------------------------------------------------|
| tokio       | wisp           | Async runtime; the entire DB/client ecosystem assumes it.          |
| httparse    | wisp           | Zero-dep, fuzzed HTTP/1.x header parser (the one hyper uses).      |
| bytes, http, http-body, tower-service | wisp, feature `tower` only | The vocabulary types of the tower ecosystem, so Wisp can be a service. Off by default. |
| pulldown-cmark | wisp-build | Markdown pages, rendered at build time. CommonMark has many edge cases; this parser is compliant, among the fastest, and only its HTML writer is on. The runtime gets nothing. |

Deliberately *not* used by default: hyper, axum, tower, serde, a TOML parser, `notify`,
a proc-macro stack (`syn`/`quote`). Things we write ourselves instead: the
HTTP/1.1 connection loop, URL/form decoding, multipart parsing, HTML
escaping, the HTTP date, the template compiler, a polling file watcher, the
dev proxy of events.

Signed cookies need SHA-256 and HMAC, which `crates/wisp/src/sign.rs` has
in about a hundred lines: fixed algorithms with published test vectors
(FIPS 180-4, RFC 4231), which its tests check, and a comparison that takes
the same time wherever the signatures differ. That keeps the runtime at two
dependencies. Anything that encrypts would use a vetted crate; we do not
write ciphers.

JSON is the same kind of thing: a fixed grammar (RFC 8259), so
`crates/wisp/src/json.rs` has a strict parser for request bodies and
`FromJson` with its checks, and `live.rs` writes JSON out
([api.md](api.md)). Apps that want serde still use it
(`serde_json::from_slice(cx.body())`, `Response::json(serde_json::to_string(&x)?)`).

Apps bring their own crates for everything else: a database driver, a
mailer, an HTTP client.

## Workspace

```
crates/wisp        runtime: HTTP server, Cx, escaping, assets, dev hooks
crates/wisp-build  compiler: route scan, .wisp parser, codegen (used from build.rs)
crates/wisp-shared what runtime, compiler and browser agree on: contexts.rs, protocol.rs, client/*.js
crates/wisp-macros #[action], #[derive(Cookie)], #[derive(Json)] and #[derive(FromJson)] (proc macros; no deps but wisp-build, for `#[validate]`'s rules)
crates/wisp-cli    `wisp new | dev | build | check | lsp | mcp | update-docs`; deploy targets
editors/           VS Code and Zed extensions, tree-sitter grammar, Prettier plugin; README per editor
examples/demo      the demo app, which is also `wisp new`'s demo template
examples/api       a JSON API, which is also `wisp new --api`
tests/app          an app that uses every feature, and the tests that run it
tests/agents       every Rust and HTML snippet of AGENTS.md, compiled
bench/             the same app in other stacks, load generator, runner (bench-run)
```

## An app

```
my-app/
  Cargo.toml          deps: wisp; build-deps: wisp-build
  build.rs            fn main() { wisp_build::run() }
  src/main.rs         wisp::main!();
  src/app.html        document shell with %wisp.head% and %wisp.body%
  src/app.css         optional; Tailwind if it contains @import "tailwindcss"
  src/app.scss        optional; Sass, instead of src/app.css
  postcss.config.*    optional; PostCSS (node) after either, or on src/app.css
  package.json        optional; npm packages (`wisp add`), for bare imports
  src/hooks.rs        optional; `init` at start, `before` every request
  src/components/...  optional; Card.wisp is <Card>
  src/routes/...      pages
  src/db.rs           optional; any src/NAME.rs is the module `NAME`
  static/...          served as-is at /
```

Each `src/NAME.rs` (but `main.rs`, `lib.rs` and `hooks.rs`) is a module of
the app with no `mod` line: Wisp compiles it as `crate::NAME`, with the
prelude in scope like a route file, and route files, `---` blocks, templates
and `src/hooks.rs` reach it as plain `NAME` (`db::find(id)`). A file that
`main.rs` or `lib.rs` declares itself (`mod db;`) is left to them, as
ordinary Rust.

### Routes

Directory names are URL segments. Files that start with `+` are route files.

| File           | Meaning                                                           |
|----------------|-------------------------------------------------------------------|
| `+page.wisp`   | The page at this path: markup, after an optional `---` block of Rust. |
| `+page.md`     | A Markdown page instead (see below); so is each `x.md`, at `x`. |
| `+page.rs`     | Optional, instead of the block: `load` and `#[action]` functions. |
| `+layout.wisp` | Wraps this page and every page below it. `<slot />` or `{@render children()}`. |
| `+layout.rs`   | Optional, instead of a block: `load` for the layout.              |
| `+error.wisp`  | Rendered for errors below this directory. Gets `status`, `message` (a sentence about the status when the error says no more than its name). |
| `+page.js`     | Optional. `load({ data, url, params, route, fetch })` in the browser. |
| `+server.rs`   | `get`/`post`/`put`/`patch`/`delete` endpoints; one that takes an `id` the path has not serves `/[id]` below, and `list` is then the folder's GET. A `#[derive(Rest)]` type in it is served whole ([api.md](api.md)). |

Segment syntax: `blog` (static), `[slug]` (param), `[[lang]]` (optional),
`[...rest]` (rest, may be empty), `(group)` (not part of the URL).

A param may name a matcher: `[id=int]`, `[[lang=locale]]`. A matcher is
`src/params/<name>.rs` with `fn matches(s: &str) -> bool`, given the
decoded segment; `int` (ASCII digits that fit a `u64`) is built in. A segment the matcher
refuses goes on to the next route, so `/[id=int]` and `/[slug]` can live
side by side. Naming a matcher that does not exist is a build error.

```rust
// src/params/locale.rs
fn matches(s: &str) -> bool {
    matches!(s, "en" | "fr" | "de")
}
```

Priority when several routes match: static segment > param with a matcher >
param > optional with a matcher > optional > rest, compared left to right
(two matchers in one place are tried in name order). Two routes that
resolve to the same pattern are a build error. `/about/` redirects (308)
to `/about`.

Also build errors: a `.wisp` file (or `page.rs`, `layout.rs`, `server.rs`)
under `src/routes` without its `+`, which would otherwise be ignored; a
top-level `_app` or `_wisp` directory, which Wisp's own files use; a
`(group)` that is not exactly one name in parentheses; a route deeper than
32 segments. Editors' swap and backup files are skipped.

`/sitemap.xml` and `/robots.txt` are made from the route tree, at no cost
to other requests: they are answered only for a GET that no route and no
file matched. The sitemap lists each page whose addresses are known (no
parameters, or `entries()`, as for `--static`; optional ones left out),
leaving out pages in a `(private)` group and pages whose markup has `<meta
name="robots" content="noindex">` (a Markdown page's `noindex: true`).
Addresses start with `SITE_URL` (env), else the request's scheme and host
(`x-forwarded-proto`, else https, http for localhost). `robots.txt` allows
everything and names the sitemap. A file of the same name in `static/`, or
a route, is served instead. `wisp build --static` writes both when
`SITE_URL` is set.

### Markdown pages

`+page.md`, and each `x.md` in a route folder (a page at `x`), is turned
into markup at build time by `pulldown-cmark` (CommonMark, tables,
strikethrough, task lists, footnotes), then compiled like a `.wisp` page:
the folder's layouts wrap it, and with no Rust in it, it is baked.

```markdown
---
title: Hello
layout: Post
date: 2026-10-01
---
Text, and a component:

<Card title="x">

**Markdown** inside, between blank lines.

</Card>
```

- Front matter is `name: value` lines (quotes optional). `title` (else the
  first `# heading`) is the `<title>`. `layout` names a component in
  `src/components` that shows the page as its children; it gets each field
  its `{@props}` declare (`&str`/`String`, `bool`, a number, `Option` of
  one). A field it requires that the page lacks, or a value of the wrong
  type, is a build error. `noindex: true` adds `<meta name="robots"
  content="noindex">`.
- `{`/`}` in text and code are written as `&#123;`/`&#125;`, so no hole
  comes from them; raw HTML (components) is the template's own.
- Fenced code is highlighted at build time by a small highlighter in
  `wisp-build` (rust, js/ts, html, css, json, bash): `<span
  class="hl-k|s|c|n|t|a">` (keyword, string, comment, number, type or tag,
  attribute) in `<pre><code class="language-x">`. The app's CSS colors
  them; other languages keep the class, unhighlighted.
- `wisp::pages("blog")` gives the `MdPage`s (`path`, `title`,
  `get("date")`) of a folder's Markdown pages, newest `date` first, for an
  index page: `{#each wisp::pages("blog") as p}<a href={p.path}>{p.title}</a>{/each}`.
  It is a `static` slice the build wrote: no I/O, no allocation.

Trailing slash: a page's address is `/about` and `/about/` gets a 308 to
it, the query kept. `wisp::trailing_slash(Always)` in `init` turns that
round (`/about` → `/about/`, for GET and HEAD of pages; endpoints and
paths with a `.` in their last segment are left as asked), and `Ignore`
serves both. The other form is matched only after its own path matched
no route, so the default costs nothing. The build warns of a literal
`href="/…"` in a template that the setting would redirect.

### Page logic

A page's Rust goes at the top of its `.wisp`, between two `---` lines:

```html
<!-- src/routes/blog/[slug]/+page.wisp -->
---
let post = db::post(&slug).await.or_404()?;

#[action]
async fn like(id: i64) {
    db::like(id).await?;
}
---
<head><title>{post.title}</title></head>
<h1>{post.title}</h1>
<form method="post" action="?/like"><button name="id" value={post.id}>Like</button></form>
```

- The block holds items (`fn`, `struct`, `use`, `static`, `const`, `impl`,
  `#[action]`s...), which go in the page's module, and statements, which
  are its load: they run for each request, before the markup renders, and
  the markup reads their names (`post`). They run in an `async` function
  that returns a `Result`, so `.await`, `?`, `return redirect("/")` and
  `return error(404, "…")` work in them. `cx` is there (`&mut Cx`), and each
  route parameter the file names is a local: `slug: String` for `[slug]`
  and `[...rest]`, `id: u64` for `[id=int]`, `Option<String>` (or
  `Option<u64>`) for `[[lang]]`. A page with no Rust at all reads them too:
  `<h1>{slug}</h1>`.
- The markup also sees `cx` (`&Cx`) in any page, layout or error page:
  `{cx.path()}`, `value={cx.input("email")}`.
- A `+page.rs` beside the `.wisp` is the other way to write the same page:
  a `load` that returns a `Data` struct (whose fields the markup reads by
  name), and the actions. A block may also hold exactly what a `+page.rs`
  would, `fn load` and `struct Data` included, but not a `fn load` and
  statements, and a page cannot have both a block and a `+page.rs`: the
  build says which line to move. A `+layout.wisp` takes a block the same
  way; its statements run while the layout renders, so they take `cx` as
  `&Cx` and cannot await or use `?` (a `+layout.rs` `load` can). Components
  and error pages take none.
- Build and type errors in a block point at the `.wisp` file and line. A
  block's text is part of the file's shape: editing it compiles again, while
  editing the markup still swaps in without a compile.

```rust
// The same page as src/routes/blog/[slug]/+page.rs:
struct Data {
    post: Post,
}

async fn load(slug: String) -> Result<Data> {
    let post = db::post(&slug).await.or_404()?;
    Ok(Data { post })
}
```

- A route file needs no `use` lines and no `pub`. Wisp includes it into a
  module of its own with `wisp::prelude` in scope (`Cx`, `Response`,
  `Result`, `error`, `redirect`, `#[action]`, the derives, ...), and its
  template is compiled inside that module, so it reads private types and
  fields. `pub` still works, and so do `use` lines (an explicit
  `use wisp::prelude::*` replaces the one Wisp adds), and `//!` docs and
  `#![…]` attributes at the top of the file. Files with CRLF line endings
  or a byte order mark build the same as any other.
- `load` is found by name, actions by the `#[action]` marker. Nothing else in the
  file is reachable from HTTP. This is deliberate: a helper function must
  never become an endpoint by accident.
- Signatures are as short as the function allows. `load`, actions and
  `+server.rs` endpoints may be `fn` or `async fn`; take `cx: &mut Cx`,
  `cx: &Cx` or no `cx`; and return their value (`Data`, `()`, `Response`)
  either plain or in a `Result` (`Result` alone is `Result<()>`). The build
  reads which from the signature and generates the matching call; rustc
  checks the types. An `#[action]` whose body uses `cx` without taking it
  gets it (`#[action]` adds `cx: &mut Cx`), so a counter's action is
  `#[action] fn increment() { cx.set_cookie("n", n + 1) }`.
- Every other parameter is an input, read from the request by its name: a
  route parameter of that name first, then the form a POST, PUT or PATCH
  sends, then the URL's query. The type says how. `T` must be there and be
  a `T` (any `FromStr`): missing is a 400 that says which field (a 422 by
  it from a JSON body, and a body that is not JSON a 400 that says where),
  sent but not a `T` a 422 by the field (a 400 from the query), and a route
  parameter that is not one a 404. `Option<T>` is `None` when
  it is missing or blank, `bool` is a checkbox (sent at all, and not
  `false`, `off` or `0`), `Vec<T>` is every value of a repeated field, and
  `&str` borrows a `String`. So `fn load(slug: String)`,
  `fn load(q: Option<String>, page: Option<u32>)` and
  `#[action] fn add(text: String, done: bool)` need no `cx` at all.
  `cx.form()` still reads anything else, files too, and `cx.input(name)`
  finds any one by name the same way (in a block's statements, say).
- `fn entries() -> Vec<…>` in a page under `[params]` lists the pages
  `wisp build --static` writes (see [deploy.md](deploy.md)).
- The build checks, against the file, that `load` returns `Data` (plain or
  in a `Result`), that every parameter but `cx` has a plain name, and that
  `#[action]` (by any path, `wisp::action` too) marks only top-level
  functions of a page.
- Errors: `?` on any `std::error::Error` gives a 500 (details only in dev).
  `return error(404, "…")` stops with that status and message, and
  `return redirect("/…")` with a 303; both are `Err`s, so they end a
  function that returns a `Result`. `Error::new(status, "…")` is the error
  itself, and `Error::redirect(status, "/…")` takes another status. Before
  `error()` returned the `Result`, code wrote `Err(error(..))`: that is now
  a `Result` inside an `Err`, so the build stops with the line and says to
  write `return error(..)` (or `Error::new` where an `Error` is wanted, as
  in `ok_or` and `map_err`).
  `Option::or_404()` is the common shortcut.
- An action returns nothing (or `Result<()>`), and then the page renders. It
  may instead return a `Response` (a CSV export, a file), sent in place of
  the page, or an `Option<Response>` to do that only sometimes.
- A form that fails validation: the action returns `invalid(field,
  problem)`, and the page renders again, as a 422, with the form still
  there: each of its inputs shows what was typed and what is wrong with it
  (see [Actions](#actions-and-wispjs)); a parameter whose type does not
  parse (`email: Email`, `age: u8`) is the same 422 by field; an action
  written without `->` returns `Result`, so it may end in `redirect(..)`;
  `{cx.problem(field)}` places one field's message elsewhere, and
  `cx.input(field)` reads what was sent. (Any
  other error from an action shows the error page.)

  ```html
  ---
  #[action]
  fn signup(name: String, email: Email) {
      if name.trim().is_empty() {
          return invalid("name", "Tell us your name");
      }
      redirect("/welcome")
  }
  ---
  <form action="?/signup">
    <input name="name">
    <input name="email">
  </form>
  ```

  For more than a message, `cx.fail(status, value)` keeps any value for the
  load, which takes it with `cx.take()`.
- `const BODY_LIMIT: usize = 20 * wisp::MB;` in a page or a
  `+server.rs` sets the largest body that route takes (the default is 1 MB,
  or `WISP_BODY_LIMIT`). A larger one is refused with a 413 as soon as its
  head arrives. The build checks that it is a `usize`, set once per route,
  and not in a layout, where it would do nothing.
- `const CACHE: u32 = 60;` in a page or a `+server.rs` keeps what a GET
  answers, as the bytes sent, for 60 seconds: a news page that changes a
  few times a minute renders once a minute per worker instead of once a
  request (Next.js calls it `revalidate`). The rules, which make it safe to
  add to any page that reads only its URL:
  - Each worker thread keeps its own, by `Host`, path and query, with no
    lock. A new process (a deploy) starts with none; a write does not clear
    it (it is kept for its time, like any cache).
  - A request with a `cookie` or `authorization` header is answered by a
    render, and nothing is kept from it: its cookie could make the page its
    own. `const CACHE_PUBLIC: u32 = 60;` instead shares the kept answer with
    those requests too, for a page that is the same for everyone.
  - Only a 200 is kept, and never one that sets a cookie or has a
    `cache-control` of `private` or `no-store`: a page can make one answer
    its own that way. What the page or endpoint set in headers is kept with
    it.
  - `before` in `src/hooks.rs` and a `+server.rs`'s `before` still run on
    every request, before the answer is looked for, so a guard or a header
    they set applies as ever.
  - A kept answer has an ETag: a client that sends it back gets a 304.
  - A page that reads a header (`accept-language`, a custom one) varies by
    it: do not `CACHE` it. Dev mode keeps nothing. At most 8 MB of answers
    a worker; past that the stale ones go, and a flood of new query strings
    costs renders, never memory.

  The build checks that it is a `const` `u32`, one of the two names, set
  once per route, and not in a layout.
- A page that reads nothing of the request needs no `CACHE`: when it and
  its layouts have no load, statements or `+page.js`, and every hole in them
  is a literal or a component's prop given as one (`<Card title="Hi" />`,
  `{#if featured}` on a flag), the build writes the whole response into the
  binary (see [Build](#build)).
- A `+server.rs` method answers with the `Response` it returns; with any
  other value, that value as JSON (`#[derive(Json)]`); with nothing, a 204.
  An `Option<Response>` that is `None` is a 404. `body: T` (a type other
  than a string) is the JSON body read as a `FromJson` type, and an error
  on a request under `/api`, or one that sent or asks for JSON, is answered
  as JSON: see [api.md](api.md).

### Templates

```html
<head><title>{data.post.title}</title></head>

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
| `{expr}`                       | escaped `Display` of `expr`; an `Option` writes its value, or nothing for `None` |
| `attr={expr}`                  | `attr="…"`, quotes added, value escaped            |
| `disabled={cond}`              | ` disabled` if `cond`, else nothing (all HTML boolean attributes) |
| `{@html expr}`                 | unescaped `Display` (you promise it is safe)       |
| `{@const x = expr}`            | `let x = expr;`                                    |
| `{#if c}…{:else if c}…{:else}…{/if}` | `if`/`else`; `if let` works as in Rust        |
| `{#each e as pat[, i]}…{:else}…{/each}` | `for`; a plain place like `data.posts` is borrowed |
| `{#match e}{:case pat}…{/match}` | `match`; a plain place is borrowed               |
| `{#await f}…{:then v}…{:catch e}…{/await}` | a page streams `v` or `e` in later ([Streaming a page](#streaming-a-page-await)) |
| `{@render children()}` or `<slot />` | layout or component slot                   |
| `{#snippet row(item, i)}…{/snippet}` | markup to render later, in this file or a component |
| `{@render row(x, 0)}`          | renders a snippet                                  |
| `<head>…</head>` or `<wisp:head>…</wisp:head>` | appended to the document head |
| `<title>…</title>` at the top level | the same as in `<head>` (not an `<svg>`'s) |
| `{cx.path()}`                  | `cx`, the request (`&Cx`), in pages, layouts, error pages |

Expressions are Rust, passed to `rustc` verbatim, so type errors are real type
errors. Inside `<script>`, `<style>` and HTML comments there are no holes, so
CSS and JS braces need no escaping. A bare `<script>` (no attributes) is the
file's client script: the build compiles it, with the file's directives, into
an ES module, so its top-level names are its state and cannot collide with
another file's (see [client.md](client.md)). A `<script>` with a `type` or
`src` is plain HTML and copied through. Comments are stripped. Whitespace runs that
contain a newline collapse to one newline, except in `<pre>`/`<textarea>`. A
block tag (`{#…}`, `{:…}`, `{/…}`, `{@const}`) alone on its line leaves no line
behind, so loops don't print blank lines between items.

Boolean attributes (`disabled`, `checked`, `selected`, `hidden`, `open`,
`required`, ...) are on when present, whatever their value, so
`disabled="false"` would disable. For them `name={cond}` takes a `bool` and
prints the bare name or nothing, and a hole in a quoted value is an error.

Escaping covers `& < > " '`, which is safe in text and in quoted attributes.
Unquoted `attr={…}` is always quoted by the compiler. There is no way to put an
Rust expression inside `<script>`; pass values by using them there by
their Rust name or as `data.x` (sent as JSON), or through `data-*`
attributes.
The one rule for client code: braces are Rust on the server, a quoted
directive value or `{:expr}` is JavaScript in the browser.
The parser tracks where in the HTML each hole lands, and refuses the places
where escaping is not enough:

- `on*` attributes and `srcdoc` (script and a whole document);
- a tag name (`<{x}>`); a bare `<` in text is written `&lt;`, so no value
  after it can make it a tag;
- a URL attribute (`href`, `src`, `action`, `formaction`, ...) whose static
  start is a `javascript:` or `vbscript:` URL, or hides its scheme behind a
  character reference (for `{:…}` browser values too);
- places a URL hides in: `to`, `from`, `values` and `by` of an SVG
  `<animate>` or `<set>` (which can set an `href`), a `<meta>`'s
  `http-equiv`, and the `content` of a `<meta http-equiv="refresh">`;
- `//` comments inside a hole, which would comment out the generated code
  after them.

When an expression decides a URL attribute's scheme (`href={link}`,
`src="{base}/x.png"`), the value is checked where it ends
(`wisp::rt::guard_url`) and one that would run script becomes
`about:invalid#blocked`, as React and Angular do. A static start that fixes
the scheme (`/p/{id}`, `https://…`, `?q=…`) costs nothing. These rules
(escaping, which attributes hold URLs, which schemes run script) live in
one file, `crates/wisp-shared/src/contexts.rs`, in a crate both `wisp` and
`wisp-build` depend on: the runtime renders by it, the compiler folds and
checks by it, and its tests hold the browser runtime to the same cases.

A block must begin and end in the same place (in text, inside one tag, in
one attribute value), and so must each branch: otherwise one branch could
leave the page inside a tag that another never opened, and `{x}` after it
would be escaped for the wrong place.

### Components

A `.wisp` file in `src/components` (at any depth) is a component, named by
its file: `Card.wisp` is `<Card>`. It declares what it takes at the top:

```html
<!-- src/components/Card.wisp -->
{@props title: &str, count: u32 = 0, featured: bool = false}
<section class="card">
  <h2>{title}{#if featured} ★{/if}</h2>
  <p>{count} items</p>
  {@render children()}
</section>
```

```html
<Card title={post.title} count={post.tags.len()} featured>
  <Badge label="new" />
</Card>
<Card title="Drafts" />
```

- Props are Rust types, and each compiles to a parameter of the component's
  render function, so rustc checks every use. A reference type is passed a
  borrow of the expression, so `title={post.title}` gives a `String` to a
  `&str`. `name="text"` is a string; `name` alone is `true` (for a `bool`
  prop). A prop with a default may be left out. `impl Display` works too.
- `{@render children()}` shows what the tag wraps, like a layout's page.
  Children are compiled in the page that uses the component, so they see
  its `data`, loop variables and `{@const}`s.
- Checked at build time, against the file that uses it: the component
  exists (with the ones that do listed if not), every prop it is given is
  one it declares, every prop without a default is given, a flag is only
  given to a `bool`, and children only go to a component that shows them.
- The name starts with a capital letter and has a lowercase one: a tag in
  all capitals (`<DIV>`) is still HTML. Components cannot go in
  `<wisp:head>`, and only components take `{@props}`.
- Component files hot-swap like any template.

Components can also be drawn by the browser (inside client blocks, or with
`{:…}` props, `bind:` and `on:`); see [client.md](client.md).

### A component kit: `wisp ui add`

```sh
wisp ui list                     # what there is
wisp ui add button dialog tabs   # into src/components, with stories
```

`wisp ui add` copies components into `src/components`: the source is in the
CLI, the copy is the app's, to change as it likes. A file already there is
the app's and stays; `--force` writes over it. Each comes with a
`Name.stories.wisp` for the workshop at `/_wisp/components`.

| Component | Use | Browser code |
|---|---|---|
| `Button` | `<Button variant="secondary" kind="submit">Save</Button>`; `href` makes it a link | none |
| `Badge` | `<Badge tone="success">Paid</Badge>` | none |
| `Card` | `<Card title="Tea">…</Card>` | none |
| `Input`, `Textarea` | `<Input label="Email" name="email" kind="email" hint="…" problem={p} />` | none |
| `Checkbox`, `Switch` | `<Switch label="Dark" name="dark" checked />` (a checkbox, `role="switch"`) | none |
| `Select` | `<Select label="Drink" name="drink"><option>Tea</option></Select>` (native) | none |
| `Accordion` | `<Accordion title="Q" group="faq">A</Accordion>` (`<details name>`) | none |
| `Dialog` | `<Dialog id="d" title="Sure?" trigger="Delete">…</Dialog>` (`<dialog>`, `commandfor`) | none |
| `Menu` | `<Menu id="m" label="Actions"><button role="menuitem">Edit</button></Menu>` (popover) | arrow keys |
| `Tabs` | `<Tabs id="t" labels={["A", "B"]}><div>…</div><div>…</div></Tabs>` | arrow keys |
| `Tooltip` | `<Tooltip id="tip" text="Saves it"><button>Save</button></Tooltip>` | `aria-describedby`, Escape |
| `Toast` | `<Toast message={flash.unwrap_or_default()} />` in the layout; scripts send `dispatchEvent(new CustomEvent('toast', { detail: 'Saved' }))` | the list |

- Props are typed, as any component's. Styles are scoped and read the
  demo's tokens with fallbacks (`var(--accent, #896ce0)`, `--panel`,
  `--line`, `--ink`, `--radius`…), so an app's `:root` restyles them all;
  `--danger`, `--success` and `--warning` are read the same way.
- Native elements first (`<dialog>`, `popover`, `<details>`, `<select>`,
  checkboxes): the browser's keyboard, focus and screen reader support,
  and no JavaScript for ten of the fourteen. The rest follow the WAI-ARIA
  patterns. A test builds all of them with no accessibility warnings.

### Scoped styles

```html
<h1>Hi</h1>
<style>
  h1, .lead { color: rebeccapurple }
  :global(body) { margin: 0 }
</style>
```

- A `<style>` without attributes in a page, layout or component is that
  file's: every element it writes gets `class="w-xxxxxx"` (six letters or
  digits from a hash of its path), and each selector gets `.w-xxxxxx` on its
  last compound that is not `:global(…)`, before any pseudo-class or
  pseudo-element: `.card p:hover` → `.card p.w-xxxxxx:hover`. Ancestors
  may come from anywhere (a layout, `<html class="dark">`); the element
  styled is this file's. A component's elements are its own, not the page's.
- `:global(x)` is `x`, unscoped. A `<style>` with any attribute
  (`<style global>`, `media="print"`) is copied as written. `@media`,
  `@supports`, `@container`, `@layer` and nesting (`&:hover`, `h2 {}` in a
  rule) are scoped inside; `@keyframes`, `@font-face` and their names stay
  global. `@import` is a build error: it goes in `src/app.css`.
- It goes at the top level (not in a block, `<template>` or `<head>`), and
  a file may have several. The class is not put on `<html>`, `<head>`,
  `<body>`, `<title>`, `<meta>`, `<link>`, `<base>`, `<script>`, `<style>`
  or `<template>`; a `class` the browser sets (`class={:x}`) keeps it.
- The CSS is appended to `/_app/app.css`, after the app's own: no other
  request. A release build embeds it; a dev build reads it from
  `.wisp/scoped.css`, which `wisp dev` rewrites on a template save and the
  browser swaps in like any CSS change, no compile.
- Cost: the class's bytes on each element, nothing at run time.

### Accessibility warnings

The parser lints each template as it reads it. `wisp check`, `wisp dev`
(on each build and template swap) and `wisp build` print them as
warnings (`! src/routes/+page.wisp:4: <img> has no alt: … (a11y-img-alt)`);
a plain `cargo build` prints them as `cargo::warning`s. They never stop a
build.

| Name | Warns about |
|---|---|
| `img-alt` | `<img>` without `alt` (`alt=""` is fine: decorative) |
| `click-events` | `on:click` on an element that is not interactive (nor a custom element, `<sl-button>`), without both a `role` and a key handler (`on:keydown`) |
| `label-control` | `<label>` with no `for` and no control inside |
| `anchor-href` | `<a>` without `href`, or `href="#"` |
| `autofocus` | `autofocus` |
| `heading-order` | a heading more than one level below the one before it in the file |
| `button-name` | `<button>` with no text, `aria-label`, `aria-labelledby` or `title` |
| `tabindex` | `tabindex` above 0 |
| `aria-attr` | an `aria-*` name that ARIA does not have |

`<!-- wisp-ignore a11y-img-alt -->` on the line before an element silences
that lint there (several names may follow). A value set by an expression
(`alt={x}`, `:alt="x"`, `{...attrs}`) counts as set. The examples have none.

### Images

`<img src="$lib/photo.jpg" alt="…">` (a file of `src/lib`) or
`src="/photo.jpg"` (one of `static/`), a JPEG, PNG or WebP with a quoted
`src`, is filled in by the compiler before it parses the template (in
`wisp-build/src/image.rs`, on the same lines):

- Always: `width` and `height` from the file's header (a small reader for
  the three formats, EXIF orientation included; no image crate), unless
  the tag sets either. The page does not shift as images load.
- `wisp build`: each image is written as WebP at up to three widths (640,
  1280, 1920, never wider than it) into `.wisp/img/<hash>-<w>.webp`, by a
  pinned cwebp (libwebp 1.6.0, downloaded once to `~/.wisp/bin` and checked
  by SHA-256, as Tailwind is; `$WISP_CWEBP` overrides it). The names are
  the content's hash, so a second build encodes nothing. The release build
  embeds them, a `src/lib` original too, served under `/_app/img/` as
  immutable, and adds `srcset`, `sizes="100vw"`, `loading="lazy"` and
  `decoding="async"`. An attribute the tag has stays as written.
- Durable: without cwebp (no network, no build for the platform, a failed
  encode) the build warns, and the tag gets no `srcset`: the original is
  served, sized. A JPEG whose EXIF turns it gets no WebP (cwebp would not
  turn it). A `$lib/` file that is not there is a build error.
- Dev serves the original (`/_app/img/lib/photo.jpg` from `src/lib`),
  adding only `width` and `height`: nothing to encode on a save.
- `<img data-wisp-raw …>` stays as written (a `$lib/` src still gets its
  URL). A `src` with a hole, or another site's, is left alone.
- Cost: none for an app without local images; a header read per image per
  build.

### Snippets

A snippet is markup a file renders more than once, or gives to a component:

```html
{#snippet row(post, i)}
  <td>{i}</td><td>{post.title}</td>
{/snippet}

<table>{#each data.posts as post, i}<tr>{@render row(post, i)}</tr>{/each}</table>

<Table rows={data.posts} {row} />
<Table rows={data.posts}>
  {#snippet row(post, i)}<td>{post.title}</td>{/snippet}
</Table>
```

```html
<!-- src/components/Table.wisp -->
{@props rows: &[Post], row: Snippet<&Post, usize>}
<table>{#each rows as r, i}<tr>{@render row(r, i)}</tr>{/each}</table>
```

- Parameters are Rust `let` patterns, typed or not: each render gives them
  their types. The body sees the names around its definition, like a
  closure. A snippet is in scope after its `{/snippet}`, to the end of the
  block it is in; it cannot render itself (a component can).
- A component takes one as a prop of type `Snippet<A, B>` (`Snippet` for
  none), which is `&dyn Fn(&mut Out, A, B)`: `{row}` or `row={row}` in its
  tag, or a `{#snippet row(…)}` among its children, and it renders it with
  `{@render row(…)}`.
- `{:@render row(x)}` has the browser draw it: the arguments are
  JavaScript, and the body uses its parameters in `{:…}` (see
  [client.md](client.md)).

### Translations

One JSON file per locale in `src/locales`, flat or nested keys:

```json
{ "cart": { "title": "Your cart",
            "items": "{count, plural, =0 {No items} one {# item} other {# items}}" },
  "hi": "Hello, {name}!" }
```

```html
<h1>{t("cart.title")}</h1>
<p>{t("cart.items", count)} {t("hi", name = user.name)}</p>
<button on:click="n++">{:t('cart.items', n)}</button>
```

- Messages are ICU's subset: `{name}`, and `{n, plural, …}` with `=N`
  cases and the locale's CLDR ones (`one`, `few`, …; `other` required, `#`
  is the count). `'{'` is a brace, `''` an apostrophe. A plural counts by
  a whole number.
- Values: one, for a message with one placeholder; else by name
  (`name = expr`, or a variable of that name alone). In a script, one, or
  an object: `t('hi', { name })`.
- Checked at build, each at its file and line: a key missing from any
  locale, a placeholder one locale has and another lacks, a case the
  language has not, an unknown key, values that do not match.
- Compiled to an index: `t("cart.title")` is a `&'static str` from a table
  per locale (it can be a `&str` prop), with values it writes as it is
  displayed. No lookup by key at run time.
- The locale: the route's `[[lang=locale]]` (a built-in matcher of the
  app's locales), then the `lang` cookie, then `Accept-Language`, then the
  default (the first file, or `wisp::default_locale("fr")?` in `init`).
  `cx.locale()` says it, `<html lang>` is set to it, and a `CACHE`d page is
  kept per locale.
- A page's scripts get only the messages they use, in its locale, with
  the page; their plurals follow `Intl.PluralRules`. `src/lib` modules
  cannot call `t`: pass them the text.
- Switchers: `{#each wisp::locales().iter() as l}<a
  href={wisp::localize(cx.path(), l)}>{l}</a>{/each}` (`/fr/about` →
  `/en/about`).

### Actions and `wisp.js`

`<form action="?/like">` posts to the `like` action: an `action` that starts
with `?/` adds `method="post"` when the form does not say. A form with no
`action` (and `method="post"`) posts to the action named `default`. A
`<button action="?/remove&id={todo.id}">` that is not in a form becomes a
form of its own, `<form method="post"><button formaction="…">`: the
one-button forms a list's delete and toggle buttons are, which work without
JavaScript. Query parameters in an action's URL are read like form fields,
so `id` above is the action's `id: u64`.

An action checks its input on its parameters, as a `FromJson` field does:
`#[action] fn add(#[validate(len = 1..=100)] text: String)` (and `min`,
`max`, `min_len`, `max_len`, `email`). Every parameter is read and checked
before the answer, so one 422 lists each that does not pass, by field
(`wisp::rt::input::read`), from a form or a JSON body. A parameter may be a
struct with `#[derive(FromJson)]` or `Rest` (`fn default(post: Post)`): its
fields are read by name, from the form (text read by the field's type, a
blank field missing) or a JSON body, and checked by its own `#[validate]`
rules (`wisp::rt::input::whole`). A value that does not pass, or `return
invalid("text", "…")`, shows the page again as a 422. There each named
`<input>`, `<textarea>` and `<select>` of the form (one posting to `?/name`,
or a `method="post"` one posting to `default`) shows what was sent
(`wisp::rt::kept`) instead of its own value (`value={post.title}`, or
`value="text"`, which becomes the same node as it is read, so it holds
wherever it is in the tag; one with a hole in it, `value="a{b}"`, is a
build error, never a value dropped; the textarea's content, a select's `value={post.kind}`, which marks the
option with that value `selected`), followed by what was wrong with it,
`<small class="problem">…</small>` (`wisp::rt::problem`); a password or
file shows its problem but is never sent back; checkboxes, radios and
hidden inputs are left alone, and so are the inputs of a component, which
has no request. A file that writes `cx.problem("text")` places that
field's message itself (`{cx.problem("text")}` is the same `<small>`, and
nothing while there is none), and gets none added for it. A GET never has
either, and takes an `if` on `cx`'s locals (none) per input, so they do
not stop a page from being baked.

The browser checks first what it can, from the same rules
(`wisp_build::rules::Native`): a page's form field that an action of the
page reads gets, as static text, the attributes whose check is one the
server makes too, so the browser never stops what the server would take:
`required` where blank is refused (a struct's field, a number, an
`Email`, an `Image`, text whose rules refuse it), `type="email"` (the
server's check is WHATWG's), `minlength` (UTF-16 units are never fewer
than characters), the most length as `pattern="[\s\S]{0,N}"` (`maxlength`
would count an emoji twice), and `min`/`max` on a `type="number"` input.
A textarea gets only `required` (its line breaks are sent as two
characters). The server still checks everything.

An action's body that has `.await` makes it `async` (`#[action]` adds the
word and the build awaits the call), so `async` is never written there.

Flow:

1. Same-origin check: if `Origin` is present it must match `Host` (403
   otherwise); without it, a `Sec-Fetch-Site` other than `same-origin` or
   `none` is a 403 too. A client that sends neither (curl) is let through.
2. The action runs. `redirect("/…")` → 303. Other errors → error page. For
   `wisp.js` (its requests carry `x-wisp`) a redirect is a 200 with
   `x-wisp-location`, and the script goes there itself: fetch would follow it
   with the post's own headers, and to another site (a payment page) not at
   all.
3. On success the page's `load` runs and the page is rendered as a normal
   response. Without JS the browser just shows it.
4. `wisp.js` intercepts the submit, sends it with `fetch`, then morphs `<body>`
   in place (keyed by `id`), so focus, scroll and unrelated inputs survive.
   A redirect to the same path updates the URL with `history.pushState`; one
   anywhere else loads that page. A response that is not HTML (a file, JSON)
   is shown as the browser would. The submit button is disabled while the
   request is out and re-enabled before the morph, so the new page decides its
   final state; a second submit of the same form meanwhile (Enter pressed
   twice) is dropped. Forms are read through attributes and
   `HTMLFormElement.prototype`, since a field named `action` or `reset` hides
   the form's property of that name. A form is sent as the browser would
   send it: a `multipart/form-data` one as multipart, files included, and any
   other urlencoded. Forms with another target are left to the browser, and
   so is a post that fails on the network.
5. After each morph the document gets a `wisp:update` event, for scripts that
   set up what the morph brought in. An element with `data-wisp-keep` is
   left as it is, children and attributes, for a widget that owns its own
   DOM (a map, a rich text editor).

### Forms and files

`cx.form()` reads either kind of form body: urlencoded, and multipart, which
is how a form with `enctype="multipart/form-data"` sends files.
`cx.form().file("photo")` is the file chosen in `<input type="file"
name="photo">` (`None` when none was), with its `name` as the browser sent it,
its `content_type` and its `bytes`; `files("photo")` is every file of a
`multiple` input. Both are visitor input: the name is never a path, and the
type says nothing the bytes do not. Text fields read the same either way.
Uploads are held in memory, so a route that takes large ones raises its own
`BODY_LIMIT` rather than the whole app's.

A picture is an action parameter: `avatar: Image` (or `Option<Image>`, which
may be left empty). It takes at most 2 MB (`wisp::MAX_SIZE`) unless
`#[validate(max_size = 5 * MB)]` says another size. The build gives its form
`enctype="multipart/form-data"` and its file input `accept="image/*"`. Its kind is what its
first bytes say, never what the browser claimed: PNG, JPEG, GIF, WebP or
AVIF. Anything else, SVG included (it can carry script), and a file over
`max_size`, show the page again as a 422 with the problem by the field. The
build adds each action's `max_size` to the page's body limit, so no
`BODY_LIMIT` is written for it. An `Image` shares its bytes when cloned, is
kept in a saved table as a `data:` URL (its JSON), and is a response as
itself: `fn get(id: u64) -> Option<Image> { USERS.get(id)?.value.avatar }`
in `avatars/[id=int]/+server.rs` sends its bytes and type with an ETag
(304 when the browser has them), `no-cache` and `nosniff`. Any response
with an `etag` answers a matching `if-none-match` GET with a 304.

To serve saved files back, `async fn get(name: String) -> Result<Response> {
Response::file_in("uploads", &name).await }` in a `[...name]/+server.rs`
reads one from the directory without blocking, typed by its extension.
`Response::download("report.csv", bytes)` sends bytes the browser saves as
a file of that name. The name may come straight from the URL: one that
would reach outside the directory (`..`, an absolute path, a drive) is a
404, like a file that does not exist.

### Cookies

State that belongs to one visitor goes in a cookie: `cx.set_cookie(name, value)`
sets it site-wide for 400 days, `HttpOnly`, `SameSite=Lax` (the value is any
`Display`; an empty one deletes it), and `cx.cookie(name)` reads it back within
the same request (`cx.cookie_or(name, default)` parses it as any `FromStr`), so
the `load` that runs after an action sees what the action stored.

`cx.set_signed_cookie(name, value)` adds an HMAC-SHA256 signature of the name
and value (`value.signature`), keyed with `WISP_SECRET`; `cx.signed_cookie(name)`
is the value only if the signature holds, so a visitor can read it but not
make one up, change it, or move it to another cookie's name. That is enough
to say who is signed in. A release build without `WISP_SECRET` fails the
request that signs or checks one, saying to set it; dev builds keep a secret
in `.wisp/secret` so sessions survive restarts. To change the secret without
signing everyone out, move the old one to `WISP_SECRET_OLD`: signatures it
made still hold (nothing new is signed with it) until it is removed, 30 days
on for sign-ins.

Signing in is built on that. For a `#[model]` with a `hash` field and an
`email` or `name` (an `Account`), `cx.signup(&USERS, row).await?` hashes the
password in `row.hash`, refuses a taken name with a 422 and signs in, and
`cx.login(&USERS, &email, &password).await?` checks it as slowly for a name
no one has; `wisp::signup` and `wisp::login` do the same without a `Cx`. `cx.sign_in(id)` (a row id of the app's users)
sets the signed cookie `session` to the id and the time, for 30 days;
`cx.signed_in()?` is the id, and signed out (or 30 days on) it is the error
that sends the visitor to sign in: a 303 to `/login`, or a 401 for a JSON
client. `cx.user(&USERS)?` is the row itself, the same way. A members' page
starts with `let me = cx.user(&USERS)?;` (`cx.user()` when `init` names the
table: `wisp::users(&db::USERS)`); `cx.signed_in().ok()` asks without
sending anyone anywhere; `cx.sign_out()` ends it. The page at `/login` is
the convention; `wisp::sign_in_page("/enter")` in `init` names another.
`sign_in` always sets a new session, so one planted on a visitor before
they sign in never becomes theirs.

A signed session is valid wherever it is sent, so `cx.sign_out()` cannot
end a copy someone stole. `wisp::sign_out_everywhere(id)` can: it counts
up the id's sign-outs in the saved table `wisp_sign_outs`, and a session
made after carries the count (`id.time.count`; none while it is 0), so
every older one no longer matches. Reading a session looks the count up
in memory, and not at all while no one has ever signed out everywhere (an
atomic flag says so). The table is read when the server starts (not at
all while it has no log file, so an app that never signs anyone out makes
none); with the app's own store (`wisp::store`), which instances can
share, a thread reads it again every 30 s and the higher count of each id
wins, so a sign-out on one instance holds on all within that. Log files
are each instance's own. A store that cannot be read leaves sessions as
they were rather than failing them, says why, and is tried again;
`sign_out_everywhere` then returns its `Err`, saying why: a security
action fails as a value, never by a panic.

Passwords are kept as `wisp::password::hash(&password).await?`, checked with
`wisp::password::check(&typed, hash).await?` (`hash` an `Option<&str>`:
`None` for no such user hashes a stand-in, as slow, so the time does not
say which names exist; a full hashing queue, about 3 s of work counted in
rounds, so hashes planted with many cannot make the wait longer, is a 503
with `retry-after`; that keeps the machine answering through a flood, and
a `RateLimit` on the sign-in action is what stops one): PBKDF2-HMAC-SHA256 on the same
HMAC, 600,000 rounds (OWASP), a random 16-byte salt, written as
`$pbkdf2-sha256$i=600000$salt$key` so the count can be raised later and old
hashes still check; `wisp::password::outdated(&hash)` says when one was made
with fewer rounds, to hash again once the password checks. A stored hash
naming more than 10,000,000 rounds is refused rather than computed. The key's padded blocks are hashed once and each round
is two SHA-256 compressions of one fixed-shape block, with no allocation:
a hash is a fraction of a second of one core, on purpose. It runs on a
thread kept for hashing (one per two cores, started with the first hash),
never the worker's: a worker held for 0.2 s would stall every connection
on its core, and a flood of sign-ins takes at most half the machine. The
edge build, which has no threads, hashes in place.

`cx.set_cookie_with(name, value, CookieOptions { … })` takes the rest: a
`max_age` (`None` ends it with the browser), `script_readable`, `same_site`
(`Lax`, `Strict`, `None`), `path`, `domain`, and `signed`. Cookies get
`Secure` when the request came over HTTPS through a proxy
(`x-forwarded-proto: https`) or `ORIGIN` is `https://`, and always with
`SameSite=None`, which browsers require.

A struct of such values goes in one cookie with `#[derive(Cookie)]`, which
writes its `Display` and `FromStr`: the fields in order, separated by `|`,
each escaped for a cookie (`42|cranesloth|pi`); an enum without fields is its
variant's name. The derive reads only the type's name and field names, with
no `syn`. What comes back is visitor input, so a type with rules beyond its
field types checks them after reading (Wisple's `Game::valid`).

A post that redirects to another page loads that page, so its scripts run as
on any load; a script that a morph brings into the same page (one inside an
`{#if}`) runs once, the first time it appears.

### Hooks, state and values for one request

`src/hooks.rs` holds what runs outside any one route:

```rust
pub struct User(pub String);

/// Once, before the server listens. A failure stops it with the reason.
async fn init() -> Result<()> {
    wisp::provide(Db::connect(&std::env::var("DATABASE_URL")?).await?);
    Ok(())
}

/// Before every page, action and endpoint (and 404), not static files.
fn before(cx: &mut Cx) -> Result<()> {
    cx.set_header("x-frame-options", "DENY");
    if let Some(name) = cx.signed_cookie("user") {
        let user = User(name.to_string());
        cx.set(user);
    }
    if cx.path().starts_with("/admin") && cx.get::<User>().is_none() {
        return redirect("/login");
    }
    Ok(())
}
```

- `before` may return nothing (or `Result<()>`), or a `Response` to send
  instead of the route (or `Option<Response>`: a CORS preflight, a
  maintenance page). Its errors and redirects are the route's would be.
  Headers it sets stay on the response even when the route fails, so
  security headers reach error pages too.
- `init` takes no `cx`: there is no request yet. It runs on the thread that
  accepts connections, whose runtime runs until the process ends, so what it
  connects (a pool) keeps being driven.
- `wisp::provide(value)` makes a value (a database pool, a client) available
  everywhere as `wisp::state::<T>()`. A type that was never provided panics
  with its name: a missing line at startup, found by the first request.
- `static TODOS: Shared<Vec<Todo>> = Shared::new(Vec::new());` is a value
  in memory that every request shares: `TODOS.lock().push(todo)`. A
  `Mutex` without the `unwrap` (a panic while it was held leaves the value
  as it was); do not hold the guard across an `.await`.
- `static TODOS: Table<Todo> = Table::new();` keeps rows under ids it
  gives: `TODOS.add(todo)` returns the id, `get(id)`, `all()` and
  `find(|t| …)` return copies as `Row { id, value }` (which reads as its
  value: `{todo}`, `todo.title`, `todo.id`), `update(id, |t| t.done = true)`
  and `remove(id)` change it. `Table::saved("todos")` also keeps them in
  the app's store (log files in `WISP_DATA`, or any database through
  `wisp::Store`), so they are there after a restart. A `#[derive(Rest)]`
  type has a saved one of its own, `Note::table()`, which a `+server.rs`
  serves (see [api.md](api.md)).
- `cx.set(value)` hands a value along the rest of one request, and
  `cx.get::<T>()` reads it: `before` finds the user once, every page reads it.
  `cx.take::<T>()` moves it out, so it need not be `Clone`.
- `cx.bearer()` is the token of an `Authorization: Bearer` header, `cx.host()`
  the `Host`, and `cx.delete_cookie(name)` removes a cookie.
- `cx.flash("Saved")` leaves a message for the next page the visitor sees
  (after a `redirect`, say), whose `load` reads it once with `cx.flashed()`.
- `src/hooks.rs` is `crate::hooks`, so routes can use its `pub` types. Like
  a route file it needs no `use` lines. The build checks it: `init` and
  `before` are shaped as above, any other `pub fn` is a mistake (a typo like
  `befor` would never run; a private one is a helper, and rustc warns when
  nothing calls it), and `main.rs` must not declare `mod hooks` itself.

### Streaming

`Response::stream(content_type, |body| async move { … })` returns a
response whose body the closure writes, in a task of its own, with a
`Sender`: each `send` goes out at once (chunked on HTTP/1.1), and the body
ends when the closure returns. `Response::events(|events| …)` is the same
for server-sent events, uncached and unbuffered by proxies, and
`Sender::event` writes one event whatever lines it has; a page listens with
`listen(url, …)` or `new EventSource(url)`. A send fails once the client
has gone, so `?` on it stops the closure. When the server stops, open
streams end properly.

```rust
// src/routes/clock/+server.rs
fn get() -> Response {
    Response::events(|events| async move {
        loop {
            events.event(&now()).await?;
            wisp::sleep(Duration::from_secs(1)).await;
        }
    })
}
```

### Streaming a page: `{#await}`

A slow part of a page need not hold the rest back:

```html
<h1>{user.name}</h1>
{#await stats(user.id)}
  <p>Counting…</p>
{:then s}
  <p>{s.posts} posts</p>
{:catch e}
  <p>No stats: {e}</p>
{/await}
```

The page goes out at once, each `{#await}`'s pending markup in place inside
a `<wisp-await>`. The response stays open (chunked), and as each future is
done its `{:then}` or `{:catch}` follows, after `</html>`, as
`<div data-wisp-await="K">…</div>` and a one-line script that moves it into
place. Answers go out in the order they come, so a quick one never waits
for a slow one. Without JS the answers stay at the end of the page, where
the browser shows them. wisp.js puts them in place itself when it navigates
to the page (and in `wisp dev`'s reloads), and the inline script has its
hash in the CSP like any other. For a client that takes gzip the stream is
gzipped a piece at a time, each piece flushed, so the page still shows
before the answers (other pages are left to a proxy; a proxy may hold a
stream back to compress it).

- The expression is a future, not awaited: `stats(id)`, `async { … }`. It
  runs after the page is sent, so it is `Send + 'static`: it owns what it
  reads (no borrowed locals), as does each branch, which also sees statics
  and the value. `{:then v}` gets a `Result`'s `Ok` value or any other
  value as it is; `{:catch e}` gets an `Err` as text (a `wisp::Error`'s
  message). `{:then}` and `{:catch}` may leave out the name, or the branch.
- A branch renders after the request, so it has no `cx` (a build error that
  says so: read what it needs before, into the future), and a form's fields
  in it show their own values, as in a component.
- Components in a branch start with the page's: their instances come with
  the answer, numbered on from the page's, and join the page's list, so
  live.js (which runs once the response has ended) starts them all, islands
  as they say. The page's own browser code (`{:x}`, `on:`, `bind:`, browser
  blocks) can't go in a branch: its instance has started without it. That
  is a build error at the await's line.
- A future that fails with no `{:catch}`, panics, or is not done within
  `WISP_HANDLER_TIMEOUT` (whatever its branches) shows `Something went
  wrong`. None of it touches the worker or the rest of the response. A
  client that leaves stops the futures.
- Only a page's own markup awaits: not a layout, component, error page,
  snippet, `<head>`, attribute, browser block or another `{#await}`. A page
  with `CACHE`, or drawn by the browser (`SSR = false`), is a build error.
  `PRERENDER`, `--static` and `--spa` wait for every answer and write it
  into the file.
- The choice is made at build, per page. A page without `{#await}` is built
  and answered exactly as before: one buffered write with its
  `content-length`, no extra branch on the way, and nothing of it in `Out`.
  The answers a render defers wait in a thread-local list that only an
  await page touches.

### Client code

Reactivity in the browser is in [client.md](client.md). In short: a bare
`<script>` per file, directives (`on:`, `bind:`, `:attr`, `class:`, `use:`,
`transition:`), `{:expr}` holes, client `{:#if}` and `{:#each}`, client
components, stores, a client router and `use:enhance`. The build tokenizes the
script (`wisp-build/src/js.rs`), finds which Rust values it uses, and sends
only those as JSON (`wisp::Json`). `live.js` (loaded only on pages that have
client code) runs it; `wisp.js` does forms and the router. No `eval`, no
`with`, nothing is required with JavaScript off.

`wisp.js` does the morph, the router and forms. Dispatching `wisp:refresh`
on the document morphs the current URL's page in again. Dev code lives in `wisp-dev.js`, which only debug builds serve
and link, so none of it ships in production pages.

### Built-in UI

Wisp draws a few things of its own, all from one design system: Kinetrix's
roles and values (light and dark, following the system), with Wisp violet
(`#7456d6` light, `#896ce0` dark) as the one accent, only on what is
interactive. One-pixel hairlines, two shadow steps, one type scale, and one
focus ring. The styles live in `crates/wisp/src/client/ui.css` (tokens,
buttons) and `dialog.css` (dev only), all `--wisp-*` tokens
and `.wisp-*` classes, so they never touch an app's own CSS.

- **The error page**, for apps without a `+error.wisp`: the status and one
  line, centered (`404 | Not Found`). The line is the status's name, or the
  error's own message when it says more. No links or buttons; an app that
  wants them writes a `+error.wisp`. Its few styles come inlined, since the
  app's own CSS may not exist yet.
- **The build error dialog** in dev: a title and one sentence saying where to
  look (`src/routes/+page.rs, line 7. Save a fix and the page updates.`), then
  the error text in a code block with a Copy control. It lives in a shadow
  root hung off `<html>`, so neither the app's CSS nor a page morph can touch
  it, and it closes by itself when the next build succeeds. While a rebuild
  runs, a two-pixel accent line crosses the top of the window, once the
  rebuild has taken 200 ms.
- **The terminal.** Every status line has a mark and words: `✓` done in
  green, `!` needs a look in yellow, `✗` failed in red, `›` under way and `~`
  changed in dim. Violet is only for what can be typed. A failure is a
  sentence, then the reason or what to do indented under it.

## Runtime

- `wisp::main!()` is `wisp::app!()` plus a `main` that calls
  `wisp::run::<App>()`; an app that sets things up first writes that `main`
  itself.
- `wisp::run::<App>()` serves on `$HOST:$PORT` (default 3000), thread per
  core: one worker per CPU (`$WISP_THREADS`), each a single-threaded tokio
  runtime with its own I/O driver, and the main thread accepting connections
  and handing them out in turn. A connection lives on one thread, so the
  request path never wakes another thread. A multi-thread tokio runtime
  funnels every socket event through one driver; measured, it left more than
  half the cores idle at under half the throughput (bench/README.md). The
  tradeoff: no work stealing, so a handler that blocks its thread stalls that
  thread's connections. Dev builds log any handler that holds its thread for
  100 ms or more in one go, with what to use instead.
- On Linux 6.1 and later each worker has an io_uring of its own
  (`crates/wisp/src/uring.rs`) and a listener of its own on the same port
  (`SO_REUSEPORT`: the kernel spreads connections, no thread hands them
  out). One `io_uring_enter` per turn of a worker submits every response
  its connections queued since the last turn and runs the completions that
  came in meanwhile (`DEFER_TASKRUN`), in place of a `recv` and a `send`
  per request. Receives stay armed for a connection's life (multishot, into
  buffers the ring lends back and forth), and accepts are one multishot
  request per worker. The ring is one more thing tokio's epoll waits on
  (through an eventfd), so handlers await timers, channels and database
  drivers as before; a WebSocket is handed to a tokio socket. At start, a
  throwaway ring receives and sends once through the workers' code, with a
  buffer ring, and one line on stderr (`wisp: io: …`) says which I/O runs
  and why not better. Where io_uring does not work (an older kernel, a
  container's seccomp profile, the `io_uring_disabled` sysctl, some 6.8
  kernels that refuse buffer rings; buffers provided per call instead
  measured slower than epoll) each worker runs the same way on an epoll of
  its own (`crates/wisp/src/epoll.rs`), its
  sockets in it edge-triggered from accept to close; a connection
  receives and sends by itself (one `recv` and one `send` a request), and
  only a send the socket has no room for is left to the driver. A
  connection is a task, but when its socket brings a request, the driver
  polls the connection's future itself, with the task's waker: a request
  whose handler does not wait is received, answered and sent without the
  scheduler, and one that waits wakes the task as usual. Which routes
  never wait is worked out at build; an `async fn before` in hooks.rs
  runs before every route, so it takes the fast path off all of them:
  keep it sync. Receive
  deadlines and stalled sends are one pass a second over the worker's
  connections, not a timer each.
  `WISP_IO=epoll` asks for that. Other systems accept on the main thread
  and hand connections out, on tokio's sockets.
- Settings, all from the environment:

  | Setting                 | What it does                                                       |
  |-------------------------|--------------------------------------------------------------------|
  | `PORT`, `HOST`          | Where to listen: 3000, on 127.0.0.1 in dev and 0.0.0.0 otherwise   |
  | `WISP_DEV`              | Dev mode: `on` in debug builds, `off` in release (5xx details, `static/` from disk, dev log) |
  | `WISP_THREADS`          | Worker threads, one per CPU by default                             |
  | `WISP_BODY_LIMIT`       | The largest request body (`1048576`, `512KB`, `10MB`); 1 MB by default |
  | `WISP_SECRET`           | Signs cookies; at least 32 characters                              |
  | `WISP_SECRET_OLD`       | The secret before, still accepted on cookies it signed (rotation)  |
  | `ORIGIN`                | The site's address (`https://example.com`), for a proxy that does not pass `Host` on |
  | `WISP_CLIENT_IP_HEADER` | The header the proxy puts the client's address in, for `cx.client_ip()` |
  | `WISP_MAX_CONNS`        | Open connections, WebSockets included, before new ones get a 503; 10000 by default, 0 for no cap |
  | `WISP_IO`               | Linux: `epoll` for an epoll per worker instead of io_uring; `uring` to fail at start, saying why, where io_uring does not work |

  They are strict: one that is set but not valid stops the server with a
  message, rather than falling back to a default. `HOST` takes an IP address
  or a name (`localhost`). A port in use, or one that needs privileges, is a
  failure with what to do.
- Behind a proxy: form posts are checked against `Host` or the proxy's
  `X-Forwarded-Host` (a page on another site cannot set that header), or
  `ORIGIN` when it is set; the first refused post logs how to fix a proxy
  that changes `Host`. `cx.client_ip()` trusts only the header
  `WISP_CLIENT_IP_HEADER` names (for `x-forwarded-for`, the entry the proxy
  added), and is the peer's address otherwise.
- On SIGTERM (how systemd, Docker and Kubernetes stop a server) or Ctrl+C,
  `run` stops accepting, answers the requests under way with
  `connection: close`, waits for the responses the drivers are still
  sending, closes idle connections (a client retries on a new one), and
  returns after at most 10 s, or at a second signal.
- Under `wisp dev` the app holds a pipe from the CLI as its stdin and exits
  when it closes, so a killed `wisp dev` never leaves an app on the port.
- The io_uring and epoll drivers are the `unsafe` modules of a native
  build: the ring's setup, the memory it shares with the kernel, and the
  socket calls std has no word for, each block with why it holds. An earlier io_uring
  prototype, which waited in `io_uring_enter` and had no deferred task work,
  measured level with plain tokio (bench/README.md). On Windows (a
  development platform for Wisp apps) tokio waits on sockets through AFD
  polls, which costs about 3 µs a request more than a completion port
  would; not worth `unsafe`.
- `wisp::serve::<App>(addr)` is the async form, for apps that must own their
  runtime. It runs until its future is dropped.
- One task per connection. `Cx` owns the connection's read buffer; the task also
  owns a write buffer and an `Out { head, body }` pair of `String`s, all reused
  across requests. A connection holds them only while it has a request: an
  idle one (tokio's or the epoll's; not yet the ring's), a WebSocket and a
  streamed response give them back to the thread's pool, so an idle
  keep-alive connection costs about 4 KB on Windows, its task and socket.
  A wakeup with nothing to read (tokio on Windows says every new socket
  is readable) gives them back again rather than waiting in a read.
- HTTP/1.1 with keep-alive and pipelining: every complete request in the read
  buffer is answered into the write buffer, then one `write_all`.
- Requests are parsed in place (`httparse`) and recorded in `Cx` as byte spans
  into its buffer, so `Cx` has no lifetime and handlers take `&mut Cx`.
- Limits: 16 KB of headers, 100 headers, a 1 MB body (`WISP_BODY_LIMIT`, or
  a route's `BODY_LIMIT`, checked against `Content-Length` before any of the
  body is read), 10 s to receive a request's head and then 10 s for each
  part of its body (an upload may take minutes as long as it keeps coming),
  60 s keep-alive idle, 30 s for a client to take any of a response. The
  buffer for a body grows as it arrives, at most 1 MB ahead: a large
  `Content-Length` alone allocates nothing. A refused request is answered,
  the sending side closed, and what the client still sends read and
  dropped for up to 2 s (and 1 MB), so the close does not reset the
  connection before the client has read why. On the wire, HTTP/1.1 without
  `Host`, or any request with two, → 400 (RFC 9112 §3.2); an absolute-form
  target (`GET http://host/x`, as a proxy sends) is its path, with its host
  as `Host` (§3.2.2); `Expect: 100-continue` is answered only to HTTP/1.1.
  Out of descriptors, accepting pauses 50 ms at a time, logged once a
  second. These deadlines and buffer rules are one module of plain
  functions (`crates/wisp/src/policy.rs`) that tokio's sockets, the epoll,
  the ring and the epoll driver's own answers all call, tested there on a
  made-up clock.
- Chunked request bodies are read, strictly: `Transfer-Encoding: chunked`
  alone (any other coding → 501), hex sizes of at most 16 digits, CRLF line
  ends, extensions and trailers skipped but bounded, and framing may not
  more than double a body's size. The data is moved together in place, so
  `cx.body()` is one slice either way. `Content-Length` with
  `Transfer-Encoding`, two `Transfer-Encoding`s, or chunked HTTP/1.0 → 400.
- A log line that cannot be written (stderr's reader gone) is dropped rather
  than panicking.
- `Date` is cached per thread and reformatted once per second.
- A panic in a handler becomes a 500 for that request; the connection survives.
- HTTP/2, TLS and compression belong to the reverse proxy / CDN (Caddy, nginx,
  Cloudflare). This keeps the binary small and the hot path simple. (Or run
  Wisp as a tower service under hyper or axum: [embed.md](embed.md).)

### One request entry point

The built-in server is one front end. `respond` decides an answer as a
`Reply { status, headers, body }`; the HTTP/1.1 writer adds `content-length`,
`date` and `connection`. Everything else calls the same code:

- `wisp::prepare::<A>()` runs `init` and sets what a request needs.
- `wisp::handle::<A>(Request) -> Reply` answers one request in process.
- `wisp::test::client::<A>()` is `handle` with cookies, for tests.
- `wisp::test::browser::<A>()` (feature `browser`) is the built-in server on
  a free port, driven in headless Chrome or Edge over the DevTools protocol
  by a small blocking WebSocket client (`crates/wisp/src/test/browser.rs`).
- `wisp::tower::service::<A>()` (feature `tower`) is a `tower::Service`.
- `wisp build --static` runs `handle` for each page and writes files.
- `wisp build --target` compiles the same code to WebAssembly
  (`crates/wisp/src/edge.rs`), driven by a small JS bridge: no wasm-bindgen.

Every path uses the same request parser and limits. See [embed.md](embed.md)
and [deploy.md](deploy.md).

### Less Rust boilerplate in templates

- `Data` fields are in scope: `{count}` for `{data.count}`.
- `class:won={data.won}` toggles a class on the server.
- `<a {href}>` is `href={href}`.
- An `Option` attribute (`aria-current={current}`) is left out when `None`.
- `#[derive(Json)]` for JSON without serde: an endpoint returns the value,
  or `Response::json_of(&value)` wraps it.

Generated code implements one trait:

```rust
pub trait App: 'static {
    fn init() -> impl Future<Output = Result<()>>;
    fn handle(route: Option<usize>, cx: &mut Cx, out: &mut Out) -> impl Future<Output = Result<()>> + Send;
    fn body_limit(route: usize) -> Option<usize>;
    // + static tables: shell, assets, templates (dev)
}
```

`handle` calls `before` from `src/hooks.rs`, then is a single `match` over
the route id, so the whole server is monomorphized with the app. There are
no handler trait objects anywhere. (Values given to `provide` and `cx.set`
are the one place with `dyn Any`: a lookup by type, off the hot path unless
the app uses them.)

## Build

`wisp_build::run()` (in the app's `build.rs`):

1. Walks `src/routes`, builds the route table, sorts by priority, rejects conflicts.
2. Parses every `.wisp` file (routes and `src/components`) into a node list.
   Errors are `file:line:col: msg`. A `---` block at the top is cut off
   first, with its lines left blank so the markup keeps its line numbers,
   and split by the same lexer into its items and its statements.
3. Scans `+page.rs`/`+layout.rs`/`+server.rs`, the blocks' items,
   `src/hooks.rs` and the app's `src/NAME.rs` modules with a tiny
   Rust lexer for `fn load`, `#[action] … fn name`, HTTP-method functions,
   hooks and `const BODY_LIMIT`, and reads from each signature whether it is
   async, takes `cx` (or, for an action, uses it without taking it), which
   inputs it reads by name, returns a `Result` and returns a `Response`.
4. Writes `$OUT_DIR/wisp.rs`: a module per user file, which `include!`s it
   after `use wisp::prelude::*` and holds a `__call` module of small shims
   that read inputs and adapt what the function returns (so the file's items
   need not be `pub`), and the template it feeds; one render function per
   template, the router `match`, `handle`, and asset tables. A `load`'s
   `Data` leaves its module in a public box (`__call::Loaded`) only that
   module's template opens, since a private type cannot travel on its own.
   A block's statements need no box: they are the start of the page's
   render function, which is `async`, and the markup is a closure after
   them that the layouts call, so it reads their locals with the types rustc
   infers. Each line of a block is written with a `// file.wisp:line`
   comment, which `wisp dev` uses to tell rustc's errors against the file.

Release builds embed `static/` and the built CSS into the binary with a content
hash, served with `Cache-Control: immutable` under `?v=hash` URLs.

The build sees every route and template, and uses that:

- A page whose output is the same for every request (no load, statements or
  `+page.js` in it or its layouts; holes that are literals, components whose
  props are literals or their literal defaults, `{#if}` on those) is baked:
  its status line, `content-type`, `content-length`, ETag and whole document
  are one `static` in the binary. Answering it is two copies and the date;
  `if-none-match` with its ETag is a 304 without hashing a byte. Hooks still
  run first. Dev mode renders it, since `wisp dev` swaps templates without a
  build, and so does a status `before` set.
- In a release build, text and literal holes next to each other are one
  `push_str`: `<p title={"a"}>{"<b>"}</p>` is `<p title="a">&lt;b&gt;</p>`,
  escaped at build time. Integers, floats and `bool` are written without
  escaping: they cannot hold markup.
- The router matches a path with no parameter in it whole, by its length
  and then its bytes (no other route that matches it can come first); only
  the rest split the path, into an array as deep as the deepest of them,
  with parameters as slices of the path.

## `wisp new`

`wisp new [name]` asks where the app goes, which template (Demo: a home page
with a counter, an about page and Wisple, a word game built on form actions;
Minimal: one page, a layout and an error page), whether to add Tailwind,
whether to create a git repository (yes unless the app lands inside one, like
`cargo new`), and whether to download and compile dependencies now. Every
question has a flag (`--template`, `--[no-]tailwind`, `--[no-]git`,
`--[no-]install`); `--yes`, or no terminal to ask on, takes the defaults.
The prompts are plain lines on std, not a cursor-driven menu.

It never writes into a directory that has anything in it, and refuses a
name Cargo would reject or that would collide with Wisp's own crates
(`build`, `deps`, `test`, `wisp`...). An app created inside another Cargo
workspace gets an empty `[workspace]` table, so it builds on its own.

Until Wisp is on crates.io, apps depend on it by path when `wisp` was built
from a clone (`cargo install --path crates/wisp-cli`), so changes to Wisp
reach them at once, and on https://github.com/wyziedevs/wisp when it was
installed with `cargo install --git`.

The demo template is `examples/demo` itself, read with `include_str!`, so the
two cannot drift. A published crate has no `examples` beside it, so build.rs
copies them into `crates/wisp-cli/templates/vendor` whenever they are there
and differ (a build in the repo refreshes it; commit the result), and a build
without them reads that copy. With Tailwind, the template's styles go in `@layer base`
after the import, so utility classes still win over them.

### AI agents

Every app is written with AGENTS.md, the whole reference in one short page,
and a pointer to it for each agent that reads a file of its own:
`CLAUDE.md`, `.github/copilot-instructions.md` and `.cursor/rules/wisp.mdc`.
The app's AGENTS.md is the repository's (embedded at build time through
the vendor copy, so it never drifts) less its part for work on Wisp, and
ends with a line after which the app's own notes go. `wisp update-docs`
brings it up to the installed Wisp, keeping those notes, and writes any
pointer file that is missing (one that is there is the app's).

Every Rust and HTML snippet in AGENTS.md is in `tests/agents`, an app in
the workspace, so building the workspace compiles them; its test fails
when one is missing there. `llms.txt` (llmstxt.org) links the docs, and
`llms-full.txt` is AGENTS.md and the docs in one file, written by a
wisp-cli test that fails when it was stale.

`wisp mcp` is a Model Context Protocol server over stdio (JSON-RPC 2.0, a
message a line, `wisp_shared::json`), for the app in the current folder:

| Tool | Answers |
|---|---|
| `wisp_docs(topic)` | the AGENTS.md or docs sections about the topic; no topic lists them |
| `wisp_check()` | `{"ok":true}` or `{"ok":false,"errors":[{file,line,col,message}]}` |
| `wisp_routes()` | each route's pattern, folder, params, page, actions and endpoints |
| `wisp_components()` | each component's name, file and props (type, default) |
| `wisp_new_route(path, kind)` | writes `+page.wisp` (default), `+layout.wisp`, `+error.wisp` or `+server.rs`; never overwrites |

Setup, in the app's folder:

- Claude Code: `claude mcp add wisp -- wisp mcp`
- Cursor: `.cursor/mcp.json` with
  `{"mcpServers":{"wisp":{"command":"wisp","args":["mcp"]}}}`
- VS Code: `code --add-mcp '{"name":"wisp","command":"wisp","args":["mcp"]}'`,
  or `.vscode/mcp.json` with
  `{"servers":{"wisp":{"type":"stdio","command":"wisp","args":["mcp"]}}}`

## Dev loop

`wisp dev` is one std-only process:

- Polls `src/`, `static/`, `Cargo.toml`, `build.rs`, `package.json` and
  `postcss.config.*` mtimes every 50 ms (no `notify`). Editors' swap, backup and lock files
  are ignored, and a burst of changes settles for at most a second.
- `wisp dev [--port <n> | --port=<n> | -p <n>]`; anything else is an error
  with the usage. The app gets `HOST=127.0.0.1` unless `HOST` is set, and the
  address it prints is the one shown and used for hot swaps (port 0 works).
- Runs Tailwind standalone `--watch` into `.wisp/app.css` if `src/app.css`
  imports Tailwind, or Dart Sass (standalone, pinned in `~/.wisp/bin`,
  `$WISP_SASS` overrides) `--watch` if `src/app.scss` exists; with a
  `postcss.config.*`, the tool writes `.wisp/pre.css` and the app's
  `node_modules/postcss-cli` (run by `node`, no npx) `--watch` makes
  `.wisp/app.css` of it (or of a plain `src/app.css`). Otherwise
  `src/app.css` is served as written. A watcher makes the first build
  itself; adding or removing `src/app.scss` or a `postcss.config.*`
  replaces the watchers. `wisp build` runs each once, minified
  (`--minify`, `--style=compressed`).
- Every child of `wisp dev` dies when its stdin, a pipe `wisp dev` holds,
  closes: however `wisp dev` ends, even killed, the system closes it. The
  app exits by itself (`exit_with_parent`); a CSS watcher runs under
  `wisp __child <exe> <args…>`, which kills the tool at that point.
- Bare imports (`'canvas-confetti'`) are npm packages: `package.json`
  pins them to exact versions (`wisp add`), dev imports
  `https://esm.sh/pkg@v?target=es2022`, and a release build serves
  `.wisp/npm`, which `wisp build` fills from esm.sh before compiling: the
  modules `wisp check` finds imported and what they import, a level at a
  time, 16 at once, only those not there yet. Each is saved with its
  imports pointed at `/_app/c/npm/`, and an import of esm.sh's re-export
  stub at the module it re-exports. The build embeds the files
  (`include_str!`); their paths name their versions, so they are cached
  for good without a `?v=`.
- Builds with `cargo build`, copies the exe to `.wisp/run/` (so the next build can
  overwrite the original while the old server keeps serving), then restarts it.
  The app is ready when it prints its `listening on` line; the CLI reads the
  app's stdout rather than polling the port (a refused connect takes 2 s to
  fail on Windows). Output is read as bytes until the pipe closes, so a line
  that is not UTF-8 never cuts the app off. The app's stdin is a pipe the
  CLI holds: if the CLI dies, even by `kill -9`, the app sees it close and
  exits, freeing the port.
- Serves a Server-Sent Events stream on its own port. Browsers stay connected
  across app restarts and are told to morph/reload once the new app is ready.
- Shows only what matters: cargo's progress on the first build, then one line
  per change (`~ src/routes/+page.wisp  swapped in 0ms`, `✓ Rebuilt in
  0.3s`). Compiler errors come through with paths relative to the app and
  without the generated modules' names (`Data`, not `page_3::Data`). An error
  rustc finds in generated code that came from a template is retold against
  the template's own line, and an action that returns a value is caught
  before compiling, against `+page.rs`.
- The app logs each request under those lines in dev (`GET /nope  404
  0.1ms`, yellow for a 4xx, red for a 5xx), and a handler's panic once, with
  where it happened. Release builds log only 5xx errors.

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

`wisp fmt [paths]` formats `.wisp` files (`--check` lists the unformatted
and fails; `wisp check` warns of them): the element tree and template blocks
two spaces a level, attribute values double-quoted, a start tag that begins
its line on one line or, past 100 columns, an attribute a line; the `---`
block through rustfmt inside a wrapper fn, with the edition of the nearest
`Cargo.toml` (the workspace's when inherited; 2024 without one), as
`cargo fmt` would; `<script>`
re-indented only; `<style>` a declaration a line when it has no strings,
comments or `url(`. Text, holes, `<pre>` and `<textarea>` are never touched.
Markup that does not balance, or that would not parse to the same template,
is left as written; formatting twice equals formatting once.
`wisp fmt --stdin [path]` formats stdin to stdout (`path` for the edition),
for editors and `editors/prettier-plugin-wisp`.

### Editors

`wisp lsp` is a language server over stdio, in the CLI: JSON-RPC framed by
hand, `wisp_shared::json` for parsing, no new dependency. Each file's app is
the nearest folder above it with `Cargo.toml` and `build.rs`.

- Problems: on open and every change, the buffer goes through the build's own
  parser and checks (`wisp_build::ide::check_file`: `---` block, template,
  component props against `src/components` as last read). On open and save,
  the whole app is checked from disk as `wisp check` does, and its problem
  shows in its file, open or not. One problem per file, as the compiler stops
  at the first. A panic in a request is answered as an error; the server
  goes on.
- Hover: a component's `{@props}`, a prop's type and default, directive and
  block docs, a route param's type.
- Go to definition: `<Card>` → its file, `'$lib/x.js'` → `src/lib/x.js`, a
  literal `href="/x"` → the route's `+page.wisp` (or `+page.rs`, `+server.rs`).
- Completion: components (with their required props), props, directives,
  `on:` events and modifiers, `{#…}` / `{:#…}` blocks, route paths in `href`.
- Formatting: `fmt.rs` on the buffer, answered as one edit of the whole
  text (none when it is formatted).

`editors/vscode` is a small extension: a TextMate grammar (HTML; Rust in the
block and `{…}`; JavaScript in `<script>`, directive values and `{:…}`; CSS in
`<style>`), snippets, format on save, **Wisp: Restart server**, and a client
(`vscode-languageclient`) that starts `wisp lsp`.

`editors/tree-sitter-wisp` is a tree-sitter grammar with the same embedding
through injections, no external scanner: flat tags (markup that does not
balance still parses), nested template blocks, code left whole. Neovim,
Helix and Zed (`editors/zed`) use it; `editors/README.md` has each editor's
setup, JetBrains, Sublime and Emacs included.

Follow-up: cheap Rust checks inside the `---` block (rust-analyzer covers
`.rs` files only).

## Security

- Escaping by default; `{@html}` is the only raw output. Component props are
  typed Rust values, so they are escaped where they are shown like any other.
- Actions are opt-in (`#[action]`), same-origin checked, and take only form
  fields — no client-supplied type names or serialized state (Livewire
  CVE-2025-54068 class).
- Signed cookies bind the signature to the cookie's name and value, and are
  compared in constant time. `WISP_SECRET` shorter than 32 characters stops
  the server at start.
- Dev endpoints exist only in debug builds and only answer loopback peers
  (behind a proxy on the same machine every peer is loopback: never serve a
  debug build). Dev mode on a non-loopback address says so at start.
- Live URL attributes (`href={:x}`, `:src="x"`) block `javascript:` and
  `vbscript:` on the server's first paint and in the browser, as `href={x}`
  does; wisp.js never follows a `javascript:` redirect or `goto`, and saves a
  posted form's attachment instead of opening it as a page of this site.
- Request size and time limits as above; no request smuggling surface
  (strict chunked parsing, CL+TE rejected).
- At most `WISP_MAX_CONNS` (10000) open connections, WebSockets included;
  past it a new one gets a 503 and is closed before it costs a task.
- URL attributes whose scheme an expression decides are checked where they
  end; `javascript:` never reaches a page (see Templates).
- Pages and error pages carry a `content-security-policy` (see below).
- `examples/demo/tests/http.rs` runs the demo's binary and sends it
  malformed, oversized, smuggling and cross-site requests, path traversal
  attempts and junk cookies, and checks every answer and that the server
  keeps answering. `tests/app` is an app that uses what the demo does not
  (hooks, state, components, uploads, signed cookies, chunked bodies, body
  limits, streamed responses), and its `tests/http.rs` checks each on the
  wire.

### Content Security Policy

Every page and error page (rendered, baked or kept by `CACHE`) gets:

```
content-security-policy: default-src 'self'; script-src 'self' 'sha256-…';
  style-src 'self' 'unsafe-inline'; img-src 'self' data: https:;
  connect-src 'self'; base-uri 'self'; form-action 'self'; frame-ancestors 'self'
```

- Wisp's own scripts are files (`wisp.js`, `live.js`, modules under
  `/_app/c/`); its JSON data block runs nothing. The only inline scripts
  are the app's (`<script defer>…</script>` in a template, or in
  `src/app.html`), and no hole can go in one, so the build hashes each
  and `script-src` lists the hashes. No nonce: the header is one string
  made after `init`, so a page costs one more header line, a baked or
  `CACHE` page stays bytes made before, and the scripts wisp.js runs after
  a navigation pass (a nonce would be the first page's). An inline script
  edited in dev takes a build, for its hash.
- Dev mode adds `https://esm.sh` (npm modules) to `script-src` and
  `connect-src`, and `wisp dev`'s reload events to `connect-src`.
- `wisp::csp("img-src 'self' https://cdn.example; font-src https://f.example")`
  in `init`: each directive replaces the default one of its name, or is
  added; `script-src` keeps the hashes (unless it has `'unsafe-inline'`,
  which a hash would turn off). `wisp::csp_off()` sends none, for an app
  that sets its own.
- Not covered: endpoints and `Response::html` (not pages), `/_wisp/docs`,
  and `wisp build --static`, whose files have no headers (the host sets
  them). A script put in by `{@html}` or an `onclick="…"` attribute does
  not run; use a file, or `on:click`.

## v0 non-goals

ORM, auth, background jobs, HTTP/2 in process, Windows services.
Each is either a library users pick or a later version. A job runner can be
started from `init` with `wisp::spawn`.

## Milestones

1. **Core** – routes, layouts, templates, load, actions, errors, static files,
   `wisp.js` morph, `wisp dev` with hot swap.  ← current
2. **Measure** – dev-loop timings; req/s and latency vs ASP.NET Core Minimal
   APIs on the same machine.
3. **Flexible** – hooks, state, components, uploads, signed cookies,
   streaming, body limits, proxies.  ← done
   **Reactive and everywhere** – client scripts, router, tower, static
   export, Docker, edge targets.  ← done
4. **v0.2** – behaviors, link boosting, docs site built with Wisp.
