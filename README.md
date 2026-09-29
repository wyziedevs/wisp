<p align="center">
  <img src="examples/demo/static/favicon.svg" width="88" height="88" alt="">
</p>

<h1 align="center">Wisp</h1>

<p align="center">A fast, fun web framework for Rust.</p>

---

Wisp builds web apps from files: a folder is a URL, a `.wisp` template is its markup, and a `+page.rs` beside it holds the Rust that loads data and handles forms. The whole app, styles and static files included, compiles into one small binary.

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

```rust
// src/routes/+page.rs
struct Data {
    count: i64,
}

fn load(cx: &mut Cx) -> Data {
    Data { count: cx.cookie_or("count", 0) }
}

#[action]
fn add(cx: &mut Cx, by: i64) {
    let count: i64 = cx.cookie_or("count", 0);
    cx.set_cookie("count", count + by);
}
```

```html
<!-- src/routes/+page.wisp -->
<h1>Clicked {count} times</h1>

<form method="post" action="?/add">
  <button name="by" value="1" disabled={count >= 10}>Click me</button>
</form>
```

No `use` lines and no `pub`: Wisp brings in what route files need. A parameter other than `cx` is read from the request by its name (a route parameter, a form field, or the query), so `by` above is the button's value. Page functions can also be `async`, and can return a `Result` so `?` works inside them; `return error(404, "No such post")` and `return redirect("/login")` stop one early (they return the `Result` themselves, so not `Err(error(..))`; the build says so if you write it). Docs, `#![…]` attributes, `use` lines and `pub` are all still fine.

## Reactivity

Braces `{…}` are Rust and run on the server. A quoted value on a directive, and `{:…}`, are JavaScript and run in the browser.

```html
<button on:click="count++" class:hot="big">Clicked {:count} times</button>
<input bind:value="name">
<p>Hello {:name}</p>

<script>
  let count = 0
  let name = data.name          // server values arrive as data.*
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
| `+page.wisp`   | The page's markup                           |
| `+page.rs`     | `load` for its data, `#[action]` for forms  |
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

Pages can also stream (server-sent events), take uploads, answer with a file, set signed cookies a visitor cannot forge, and share values like a database pool through `wisp::provide`. [docs/design.md](docs/design.md) shows how.

## APIs

An API is a folder of `+server.rs` files, beside the pages or on its own (`wisp new my-api --api`):

```rust
// src/routes/api/notes/+server.rs
#[derive(FromJson)]
struct NewNote {
    #[validate(min_len = 1, max_len = 200)]
    title: String,
}

fn post(body: NewNote) -> Response {
    Response::created(&db::add(body.title))
}
```

A JSON body that does not pass comes back as a 422 listing every problem by field, and every error under `/api` is JSON. CORS and API keys are a line in `src/hooks.rs`; rate limits, channels for live updates, background jobs, an OpenAPI document with a page to try it (`/_wisp/docs`) and JSON test helpers are built in. See [docs/api.md](docs/api.md).

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

- [docs/design.md](docs/design.md): the template language, routing, actions, the runtime and the dev server.
- [docs/client.md](docs/client.md): scripts, directives, components, stores, the router.
- [docs/api.md](docs/api.md): JSON APIs, validation, CORS, auth, rate limits, channels, jobs, OpenAPI.
- [docs/embed.md](docs/embed.md): testing, `wisp::handle`, axum, hyper, Lambda.
- [docs/deploy.md](docs/deploy.md): static, Docker and every host.

## License

[MIT](LICENSE)
