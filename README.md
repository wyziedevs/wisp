<p align="center">
  <img src="examples/demo/static/favicon.svg" width="88" height="88" alt="">
</p>

<h1 align="center">Wisp</h1>

<p align="center">A fast, fun web framework for Rust.</p>

---

Wisp builds web apps from files: a folder is a URL, and its `+page.wisp` is the page, markup after a short block of the Rust that loads data and handles forms. The whole app, styles and static files included, compiles into one small binary.

- **Few tokens.** Apps are short to write, for people and for AI: pages and APIs take fewer tokens in Wisp than in SvelteKit, Next.js, Nuxt, Axum, FastAPI or Rails: 39% fewer than the next best, in all ([docs/tokens.md](docs/tokens.md)). [AGENTS.md](AGENTS.md) is the whole reference on one page.

- **Fast to build.** Markup edits appear in the browser in under 100 ms, without a recompile.
- **Fast to run.** Templates compile to plain Rust, with no virtual DOM and no runtime template engine.
- **Works without JavaScript.** Forms are real forms, file uploads included. A 7 KB script makes them update the page in place.
- **Type checked.** Every expression in a template is checked by the Rust compiler.
- **Reactive when you want it.** Add a `<script>` and a few directives, no build step.
- **Deploy anywhere.** One binary, a Docker image, static files, or the edge.

## Quick start

You need [Rust](https://rustup.rs) 1.88 or later.

```sh
cargo install --git https://github.com/wyziedevs/wisp wisp-cli
wisp new my-app
cd my-app
wisp dev
```

`wisp new` asks a few questions: a demo app, a blank one or a JSON API, Tailwind CSS, git, and whether to compile dependencies now. Then open http://127.0.0.1:3000.

## A page

```html
<!-- src/routes/+page.wisp -->
---
let count: i64 = cx.cookie_or("count", 0);

#[action]
fn add(by: i64) {
    let count: i64 = cx.cookie_or("count", 0);
    cx.set_cookie("count", count + by);
}
---
<h1>Clicked {count} times</h1>

<form action="?/add">
  <button name="by" value="1" disabled={count >= 10}>Click me</button>
</form>
```

Between the `---` lines is Rust. Its statements run for each request, and the markup reads their names (`count`); `cx` is the request. Its functions are the page's own: an `#[action]` handles a form post (it gets `cx` when it uses it), and each parameter is read from the request by its name (a route parameter, a form field, or the query), so `by` above is the button's value. No `use` lines and no `pub`. Statements can `.await`, use `?`, and stop early with `return error(404, "No such post")` or `return redirect("/login")`; so can an action, with no `->`, ending in `redirect("/")` or `;`. The markup can await too: `{#each db::items().await as item}` runs before the page renders, with no block at all. A route parameter is already a local: `[slug]/+page.wisp` can just say `<h1>{slug}</h1>`.

A form whose `action` is `?/name` posts to that action. One that does not pass shows the page again as a 422, keeping what was typed in its inputs, each followed by what was wrong with it (or `{cx.problem("email")}` where you want the message): `#[validate(len = 1..=100)] text: String` on the action's parameter checks it (so does its type: `email: Email`, `age: u8`), and `return invalid("email", "is missing its @")` says anything else. The Rust can also live in a `+page.rs` beside the page, with a `load` that returns a `Data` struct.

## Reactivity

Braces `{…}` are Rust and run on the server. A quoted value on a directive, and `{:…}`, are JavaScript and run in the browser.

```html
<button on:click="count++" class:hot="big">Clicked {:count} times</button>
<input bind:value="name">
<p>Hello {:name}</p>

<script>
  let count = 0
  let name = data.name          // server values: by their Rust name, or data.*
  let big = $derived(count > 5) // follows count; only what reads it redraws
</script>
```

Turn JavaScript off and the server's HTML still works. Lists and conditions (`{:#each}`, `{:#if}`), runes, islands (`<Chart client:visible />`), client components, stores, a client router, `use:enhance` and `+page.js` are in [docs/client.md](docs/client.md).

Less boilerplate on the server side too:

```html
<p>{count} items</p>                       <!-- a field of Data -->
<div class="grid" class:won={data.won}>
<a {href}>Home</a>
```

## Routes

| File           | Purpose                                     |
| -------------- | ------------------------------------------- |
| `+page.wisp`   | The page: a `---` block of Rust, then markup |
| `+page.rs`     | Optional instead of the block: `load`, `#[action]`s |
| `+page.js`     | Optional `load` that runs in the browser    |
| `+layout.wisp` | Wraps this page and every page below it     |
| `+error.wisp`  | Shown when something below it fails         |
| `+server.rs`   | Endpoints: `get`, `post`, ... A value they return is sent as JSON |

Folders named `[slug]` are parameters, `[[lang]]` optional ones, `[...rest]` match the rest of the path, and `(group)` folders organize routes without changing the URL. `[id=int]` only matches digits (that fit a `u64`); `[slug=word]` uses your own matcher in `src/params/word.rs`.

Beside the routes:

| File                         | Purpose                                                          |
| ---------------------------- | ---------------------------------------------------------------- |
| `src/components/Card.wisp`   | A component, used as `<Card title={x}>…</Card>`, with typed props |
| `src/hooks.rs`               | `init` runs once at start; `before` runs before every request    |
| `src/db.rs`                  | Any module of your own, no `mod` line: routes call `db::find(id)` |

Pages can also stream (server-sent events), take uploads, answer with a file, set signed cookies a visitor cannot forge, and share values like a database pool through `wisp::provide`. [docs/design.md](docs/design.md) shows how.

## APIs

An API is a folder of `+server.rs` files, beside the pages or on its own (`wisp new my-api --api`). This one is a whole JSON CRUD API for notes, saved across restarts:

```rust
// src/routes/api/notes/+server.rs
#[derive(Rest)]
#[rest(write = "API_KEY")]
struct Note {
    #[validate(len = 1..=200)]
    title: String,
    done: bool,
}
```

GET and POST `/api/notes`, GET, PUT, PATCH and DELETE `/api/notes/[id]`, writes only with `Authorization: Bearer $API_KEY`; lists take filters, sorting and pages (`?done=false&sort=-id&limit=20`), rows carry ETags, and hooks such as `fn before_create(note: &mut Note)` go in the same file. Rows live in log files by default, or in any database through `wisp::Store`. For queries of your own, write the handlers instead: `fn get(id: u64) -> Option<Note>` (an `id` puts it at `/api/notes/[id]`, `None` is a 404), `fn post(body: New) -> Response`, `fn list()`. A JSON body that does not pass comes back as a 422 listing every problem by field, and every error under `/api` is JSON. CORS and API keys are a line in `src/hooks.rs`; rate limits, channels for live updates, background jobs, webhook signatures, idempotency keys, error codes, an OpenAPI document with a page to try it (`/_wisp/docs`), a typed TypeScript client (`wisp build --client ts`) and JSON test helpers are built in. See [docs/api.md](docs/api.md).

## Tokens

The same five features (a list page, a validated form, a JSON endpoint, a layout with a nav, a live search) as a whole app in each stack, counted by `cargo run -p wisp-tokens` ([bench/README.md](bench/README.md#tokens)):

| Stack | Tokens | vs Wisp |
|---|---:|---:|
| **Wisp** | **462** | 1.0x |
| SvelteKit | 928 | 2.0x |
| Next.js | 934 | 2.0x |
| Axum + askama | 1330 | 2.9x |
| Actix + tera | 1457 | 3.2x |

## Commands

| Command      | What it does                                         |
| ------------ | ---------------------------------------------------- |
| `wisp new`   | Create an app                                        |
| `wisp dev`   | Run it with hot reload                               |
| `wisp build` | Build one release binary with everything inside      |
| `wisp build --static` | Write the site as static files              |
| `wisp build --docker` | Write a Dockerfile                          |
| `wisp build --target <host>` | Build for Cloudflare, Deno, Vercel, Netlify or Node |
| `wisp check` | Check routes and templates without compiling         |

## Deploy

```sh
wisp build                        # one binary
wisp build --static               # dist/, for GitHub Pages, S3 ...
wisp build --docker               # Fly.io, Railway, Render, Cloud Run ...
wisp build --target cloudflare    # or deno, vercel, netlify, node
```

| Host | How |
|---|---|
| VPS, any server | binary |
| Fly.io, Railway, Render, Cloud Run, Azure Container Apps | `--docker` |
| GitHub Pages, GitLab Pages | `--static` |
| Cloudflare, Deno Deploy, Vercel, Netlify | `--target` |
| AWS Amplify, Firebase, Azure Static Web Apps, Stormkit, Zeabur | `--target node` |
| AWS Lambda | `tower` feature |

Commands per host, and what the edge can't do, are in [docs/deploy.md](docs/deploy.md).

## Testing and other Rust code

Test an app in process, with no port:

```rust
let mut app = wisp::test::client::<App>();
assert!(app.get("/").text().contains("Welcome"));
```

With the `tower` feature Wisp is a `tower::Service`, so it runs inside axum, behind tower layers, or on Lambda. See [docs/embed.md](docs/embed.md).

## Performance

On a server-rendered HTML benchmark on Linux, Wisp serves 13% more requests than Actix Web, 27% more than Axum and 4.4× Fiber, in 3 MB of memory, with no `unsafe` code. See [bench](bench/README.md) for the method and full results.

## Documentation

- [AGENTS.md](AGENTS.md): everything on one page, for coding agents (and people in a hurry).
- [docs/design.md](docs/design.md): the template language, routing, actions, the runtime and the dev server.
- [docs/tokens.md](docs/tokens.md): what the same apps cost in tokens here and in other frameworks.
- [docs/client.md](docs/client.md): scripts, directives, components, stores, the router.
- [docs/api.md](docs/api.md): JSON APIs, validation, CORS, auth, rate limits, channels, jobs, OpenAPI.
- [docs/embed.md](docs/embed.md): testing, `wisp::handle`, axum, hyper, Lambda.
- [docs/deploy.md](docs/deploy.md): static, Docker and every host.

## License

[MIT](LICENSE)
