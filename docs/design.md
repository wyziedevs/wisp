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
4. **Safe.** No `unsafe` anywhere in Wisp or in the code it generates; the
   workspace lint is `forbid`. (The benchmark runner, which pins processes to
   CPUs through the OS, is the one crate that sets its own.)
5. **Mistakes fail early, in the user's own file.** Whatever the build can
   check, it checks, and says where and what to do: a private `load`, an
   `#[action]` in the wrong place, `page.wisp` without its `+`, a block that
   leaves a tag open in one branch. rustc should only ever point at code the
   user wrote.
6. **Fast dev loop.** Editing markup never waits for `cargo`. Editing Rust
   rebuilds only the app crate.
7. **Works without JavaScript.** Forms are real forms and links are real links.
   `wisp.js` enhances them; it is never required.

## Dependency budget

| Crate       | Used by        | Why it exists                                                      |
|-------------|----------------|--------------------------------------------------------------------|
| tokio       | wisp           | Async runtime; the entire DB/client ecosystem assumes it.          |
| httparse    | wisp           | Zero-dep, fuzzed HTTP/1.x header parser (the one hyper uses).      |
| bytes, http, http-body, tower-service | wisp, feature `tower` only | The vocabulary types of the tower ecosystem, so Wisp can be a service. Off by default. |

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

Apps bring their own crates for everything else: serde for JSON
(`serde_json::from_slice(cx.body())`, `Response::json(serde_json::to_string(&x)?)`),
a database driver, a mailer.

## Workspace

```
crates/wisp        runtime: HTTP server, Cx, escaping, assets, dev hooks
crates/wisp-build  compiler: route scan, .wisp parser, codegen (used from build.rs)
crates/wisp-macros #[action], #[derive(Cookie)] and #[derive(Json)] (proc macros, no deps)
crates/wisp-cli    `wisp new | dev | build | check`; deploy targets
examples/demo      the demo app, which is also `wisp new`'s demo template
tests/app          an app that uses every feature, and the tests that run it
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
  src/hooks.rs        optional; `init` at start, `before` every request
  src/components/...  optional; Card.wisp is <Card>
  src/routes/...      pages
  static/...          served as-is at /
```

Other modules of the app's own (`mod db;` in `main.rs`) are ordinary Rust,
reachable from routes as `crate::db`.

### Routes

Directory names are URL segments. Files that start with `+` are route files.

| File           | Meaning                                                           |
|----------------|-------------------------------------------------------------------|
| `+page.wisp`   | Markup for the page at this path.                                 |
| `+page.rs`     | `load` for the page and its `#[action]` functions.                |
| `+layout.wisp` | Wraps this page and every page below it. `{@render children()}`.  |
| `+layout.rs`   | `load` for the layout.                                            |
| `+error.wisp`  | Rendered for errors below this directory. Gets `status`, `message` (a sentence about the status when the error says no more than its name). |
| `+page.js`     | Optional. `load({ data, url, fetch })` in the browser on navigation. |
| `+server.rs`   | `pub async fn get/post/put/patch/delete` endpoints.               |

Segment syntax: `blog` (static), `[slug]` (param), `[[lang]]` (optional),
`[...rest]` (rest, may be empty), `(group)` (not part of the URL).

Priority when several routes match: static segment > param > optional > rest,
compared left to right. Two routes that resolve to the same pattern are a build
error. `/about/` redirects (308) to `/about`.

Also build errors: a `.wisp` file (or `page.rs`, `layout.rs`, `server.rs`)
under `src/routes` without its `+`, which would otherwise be ignored; a
top-level `_app` or `_wisp` directory, which Wisp's own files use; a
`(group)` that is not exactly one name in parentheses; a route deeper than
32 segments. Editors' swap and backup files are skipped.

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
    let id: i64 = cx.form().parse("id")?;
    db::like(id).await?;
    Ok(())
}
```

- `load` is found by name, actions by the `#[action]` marker. Nothing else in the
  file is reachable from HTTP. This is deliberate: a helper `pub async fn` must
  never become an endpoint by accident.
- Signatures are as short as the function allows. `load`, actions and
  `+server.rs` endpoints may be `fn` or `async fn`; take `cx: &mut Cx`,
  `cx: &Cx` or nothing; and return their value (`Data`, `()`, `Response`)
  either plain or in a `Result`. The build reads which from the signature and
  generates the matching call; rustc checks the types. A counter is
  `pub fn load(cx: &mut Cx) -> Data` and `#[action] pub fn increment(cx: &mut Cx)`.
- `pub fn entries() -> Vec<…>` in a `+page.rs` under `[params]` lists the pages
  `wisp build --static` writes (see [deploy.md](deploy.md)).
- `Data` must be a public type in `+page.rs` (defined or re-exported) with
  public fields, because the template reads it. The build checks, against
  `+page.rs`, that `load` and every action are `pub`, that `load` returns
  `Data` (plain or in a `Result`), that a `Data` defined there is `pub`, and
  that `#[action]` (by any path, `wisp::action` too) marks only top-level
  functions of a `+page.rs`.
- Errors: `?` on any `std::error::Error` gives a 500 (details only in dev).
  `error(404, "…")` and `redirect("/…")` (303) construct control-flow errors;
  `Error::redirect(status, "/…")` takes another status. `Option::or_404()` is
  the common shortcut. `cx.form().parse("id")` reads a field as any `FromStr`
  type, and a missing or unparsable field is a 400 that says why.
- An action returns nothing (or `Result<()>`), and then the page renders. It
  may instead return a `Response` (a CSV export, a file), sent in place of
  the page, or an `Option<Response>` to do that only sometimes.
- A form that fails validation: the action sets a status and hands what
  went wrong to `load`, which runs next in the same request, and the page
  shows it with what was typed (`cx.form()` still has it):

  ```rust
  pub struct Problem(pub &'static str);

  #[action]
  pub fn signup(cx: &mut Cx) {
      if cx.form().get("email").is_none_or(|e| !e.contains('@')) {
          cx.set_status(422);
          cx.set(Problem("That email address is missing its @"));
      }
  }

  pub fn load(cx: &mut Cx) -> Data {
      Data { problem: cx.get::<Problem>().map(|p| p.0), email: cx.form().get("email").unwrap_or_default().into_owned() }
  }
  ```
- `pub const BODY_LIMIT: usize = 20 * wisp::MB;` in a `+page.rs` or
  `+server.rs` sets the largest body that route takes (the default is 1 MB,
  or `WISP_BODY_LIMIT`). A larger one is refused with a 413 as soon as its
  head arrives. The build checks that it is a `pub` `usize`, set once per
  route, and not in a layout, where it would do nothing.

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
| `disabled={cond}`              | ` disabled` if `cond`, else nothing (all HTML boolean attributes) |
| `{@html expr}`                 | unescaped `Display` (you promise it is safe)       |
| `{@const x = expr}`            | `let x = expr;`                                    |
| `{#if c}…{:else if c}…{:else}…{/if}` | `if`/`else`; `if let` works as in Rust        |
| `{#each e as pat[, i]}…{:else}…{/each}` | `for`; a plain place like `data.posts` is borrowed |
| `{#match e}{:case pat}…{/match}` | `match`; a plain place is borrowed               |
| `{@render children()}`         | layout slot                                        |
| `<wisp:head>…</wisp:head>`     | appended to the document head                      |

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
Rust expression inside `<script>`; pass values by using them there as
`data.x` (sent as JSON) or through `data-*` attributes.
The one rule for client code: braces are Rust on the server, a quoted
directive value or `{:expr}` is JavaScript in the browser.
The parser tracks where in the HTML each hole lands, and refuses the places
where escaping is not enough:

- `on*` attributes and `srcdoc` (script and a whole document);
- a tag name (`<{x}>`); a bare `<` in text is written `&lt;`, so no value
  after it can make it a tag;
- a URL attribute (`href`, `src`, `action`, `formaction`, ...) whose static
  start is a `javascript:` or `vbscript:` URL, or hides its scheme behind a
  character reference;
- `//` comments inside a hole, which would comment out the generated code
  after them.

When an expression decides a URL attribute's scheme (`href={link}`,
`src="{base}/x.png"`), the value is checked where it ends
(`wisp::rt::guard_url`) and one that would run script becomes
`about:invalid#blocked`, as React and Angular do. A static start that fixes
the scheme (`/p/{id}`, `https://…`, `?q=…`) costs nothing.

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
`{:…}` props, `bind:` and `on:`); see [client.md](client.md). Snippets
(markup reused within one file) are v0.2.

### Actions and `wisp.js`

`<form method="post" action="?/like">` posts to the `like` action; a form with no
`action` posts to the action named `default`. Flow:

1. Same-origin check: if `Origin` is present it must match `Host` (403 otherwise).
2. The action runs. `Err(redirect)` → 303. Other errors → error page. For
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

To serve saved files back, `Response::file_in("uploads", cx.param("name")).await?`
in a `[...name]/+server.rs` reads one from the directory without blocking,
typed by its extension. The name may come straight from the URL: one that
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
in `.wisp/secret` so sessions survive restarts.

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
use wisp::prelude::*;

pub struct User(pub String);

/// Once, before the server listens. A failure stops it with the reason.
pub async fn init() -> Result<()> {
    wisp::provide(Db::connect(&std::env::var("DATABASE_URL")?).await?);
    Ok(())
}

/// Before every page, action and endpoint (and 404), not static files.
pub fn before(cx: &mut Cx) -> Result<()> {
    cx.set_header("x-frame-options", "DENY");
    if let Some(name) = cx.signed_cookie("user") {
        let user = User(name.to_string());
        cx.set(user);
    }
    if cx.path().starts_with("/admin") && cx.get::<User>().is_none() {
        return Err(redirect("/login"));
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
- `cx.set(value)` hands a value along the rest of one request, and
  `cx.get::<T>()` reads it: `before` finds the user once, every page reads it.
- `src/hooks.rs` is `crate::hooks`, so routes can use its types. The build
  checks it: `init` and `before` are `pub` and shaped as above, any other
  `pub fn` is a mistake (a typo like `befor` would never run), and
  `main.rs` must not declare `mod hooks` itself.

### Streaming

`Response::stream(content_type)` returns a response and a `Sender` for its
body: each `send` goes out at once (chunked on HTTP/1.1), and the body ends
when the sender is dropped. `Response::events()` is the same for
server-sent events, uncached and unbuffered by proxies, and `Sender::event`
writes one event whatever lines it has; a page listens with `new
EventSource(url)`. A send fails once the client has gone, which is the
signal to stop. When the server stops, open streams end properly.

```rust
// src/routes/clock/+server.rs
pub fn get() -> Response {
    let (res, events) = Response::events();
    tokio::spawn(async move {
        while events.event(&now()).await.is_ok() {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    res
}
```

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
buttons, the error page) and `dialog.css` (dev only), all `--wisp-*` tokens
and `.wisp-*` classes, so they never touch an app's own CSS.

- **The error page**, for apps without a `+error.wisp`. It is told in three
  parts: what happened (the status's name), what it means or what to do (the
  error's message, or a sentence about the status when the message says no
  more than its name), and the status with the request (`404 · GET
  /nope?x=1`) as the reference line. A 5xx carries the failure glyph and a
  Try Again button; a 4xx stays gray. Its styles come inlined, since the
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
- Settings, all from the environment:

  | Setting                 | What it does                                                       |
  |-------------------------|--------------------------------------------------------------------|
  | `PORT`, `HOST`          | Where to listen: 3000, on 127.0.0.1 in debug builds and 0.0.0.0 in release |
  | `WISP_THREADS`          | Worker threads, one per CPU by default                             |
  | `WISP_BODY_LIMIT`       | The largest request body (`1048576`, `512KB`, `10MB`); 1 MB by default |
  | `WISP_SECRET`           | Signs cookies; at least 32 characters                              |
  | `ORIGIN`                | The site's address (`https://example.com`), for a proxy that does not pass `Host` on |
  | `WISP_CLIENT_IP_HEADER` | The header the proxy puts the client's address in, for `cx.client_ip()` |

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
  `connection: close`, closes idle connections (a client retries on a new
  one), and returns after at most 10 s, or at a second signal.
- Under `wisp dev` the app holds a pipe from the CLI as its stdin and exits
  when it closes, so a killed `wisp dev` never leaves an app on the port.
- On Linux, the plain tokio worker measured level with hand-written epoll
  and io_uring servers (bench/README.md), so there is no I/O code of our
  own. On Windows (a development platform for Wisp apps) tokio waits on
  sockets through AFD polls, which costs about 3 µs a request more than a
  completion port would; not worth `unsafe`.
- `wisp::serve::<App>(addr)` is the async form, for apps that must own their
  runtime. It runs until its future is dropped.
- One task per connection. `Cx` owns the connection's read buffer; the task also
  owns a write buffer and an `Out { head, body }` pair of `String`s, all reused
  across requests.
- HTTP/1.1 with keep-alive and pipelining: every complete request in the read
  buffer is answered into the write buffer, then one `write_all`.
- Requests are parsed in place (`httparse`) and recorded in `Cx` as byte spans
  into its buffer, so `Cx` has no lifetime and handlers take `&mut Cx`.
- Limits: 16 KB of headers, 64 headers, a 1 MB body (`WISP_BODY_LIMIT`, or
  a route's `BODY_LIMIT`, checked against `Content-Length` before any of the
  body is read), 10 s to receive a request's head and then 10 s for each
  part of its body (an upload may take minutes as long as it keeps coming),
  60 s keep-alive idle, 30 s for a client to take any of a response. The
  buffer for a body grows as it arrives, at most 1 MB ahead: a large
  `Content-Length` alone allocates nothing.
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
- `#[derive(Json)]` and `Response::json_of(&value)` for JSON without serde.

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
   Errors are `file:line:col: msg`.
3. Scans `+page.rs`/`+layout.rs`/`+server.rs` and `src/hooks.rs` with a tiny
   Rust lexer for `fn load`, `#[action] … fn name`, HTTP-method functions,
   hooks and `const BODY_LIMIT`, and reads from each signature whether it is
   async, takes `cx`, returns a `Result` and returns a `Response`.
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

It never writes into a directory that has anything in it, and refuses a
name Cargo would reject or that would collide with Wisp's own crates
(`build`, `deps`, `test`, `wisp`...). An app created inside another Cargo
workspace gets an empty `[workspace]` table, so it builds on its own.

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
  `notify`). Editors' swap, backup and lock files are ignored, and a burst of
  changes settles for at most a second.
- `wisp dev [--port <n> | --port=<n> | -p <n>]`; anything else is an error
  with the usage. The app gets `HOST=127.0.0.1` unless `HOST` is set, and the
  address it prints is the one shown and used for hot swaps (port 0 works).
- Runs Tailwind standalone `--watch` into `.wisp/app.css` if `src/app.css`
  imports Tailwind; otherwise `src/app.css` is served as written.
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

## Security

- Escaping by default; `{@html}` is the only raw output. Component props are
  typed Rust values, so they are escaped where they are shown like any other.
- Actions are opt-in (`#[action]`), same-origin checked, and take only form
  fields — no client-supplied type names or serialized state (Livewire
  CVE-2025-54068 class).
- Signed cookies bind the signature to the cookie's name and value, and are
  compared in constant time. `WISP_SECRET` shorter than 32 characters stops
  the server at start.
- Dev endpoints exist only in debug builds and only answer loopback peers.
- Request size and time limits as above; no request smuggling surface
  (strict chunked parsing, CL+TE rejected).
- URL attributes whose scheme an expression decides are checked where they
  end; `javascript:` never reaches a page (see Templates).
- `examples/demo/tests/http.rs` runs the demo's binary and sends it
  malformed, oversized, smuggling and cross-site requests, path traversal
  attempts and junk cookies, and checks every answer and that the server
  keeps answering. `tests/app` is an app that uses what the demo does not
  (hooks, state, components, uploads, signed cookies, chunked bodies, body
  limits, streamed responses), and its `tests/http.rs` checks each on the
  wire.

## v0 non-goals

ORM, auth, background jobs, i18n, WebSockets, HTTP/2 in process, Windows
services. Each is either a library users pick or a later
version. Server-sent events cover pushing updates to a page; a job runner
can be started from `init` with `tokio::spawn`.

## Milestones

1. **Core** – routes, layouts, templates, load, actions, errors, static files,
   `wisp.js` morph, `wisp dev` with hot swap.  ← current
2. **Measure** – dev-loop timings; req/s and latency vs ASP.NET Core Minimal
   APIs on the same machine.
3. **Flexible** – hooks, state, components, uploads, signed cookies,
   streaming, body limits, proxies.  ← done
   **Reactive and everywhere** – client scripts, router, tower, static
   export, Docker, edge targets.  ← done
4. **v0.2** – snippets, behaviors, link boosting, param matchers, docs site
   built with Wisp.
