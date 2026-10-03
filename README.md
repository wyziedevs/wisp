<p align="center">
  <img src="examples/demo/static/favicon.svg" width="88" height="88" alt="">
</p>

<h1 align="center">Wisp</h1>

<p align="center">A fast, fun web framework for Rust.</p>

Wisp is the framework for the AI age: fast to run, cheap in tokens, and durable. A folder is a URL, and its `+page.wisp` is the page: a short block of Rust that loads data and handles forms, then markup. The whole app, styles and static files included, compiles into one small binary.

## Quick start

You need [Rust](https://rustup.rs) 1.88 or later.

```sh
cargo install --git https://github.com/wyziedevs/wisp wisp-cli
wisp new my-app
cd my-app
wisp dev
```

Then open http://127.0.0.1:3000.

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

Between the `---` lines is Rust that runs for each request; the markup reads its names, and `cx` is the request.

## Features

- **Few tokens.** A whole app is about half the tokens of SvelteKit or Next.js, and 39% fewer than the next best stack ([docs/tokens.md](docs/tokens.md)).
- **Fast to build.** Markup edits show in the browser in under 100 ms, without a recompile.
- **Fast to run.** Templates compile to plain Rust, with no virtual DOM.
- **Works without JavaScript.** Forms are real forms, uploads included.
- **Type checked.** The Rust compiler checks every template expression.
- **Reactive when you want it.** Add a `<script>` and a few directives.
- **JSON APIs.** `#[derive(Rest)]` gives a CRUD API, with validation and OpenAPI.
- **Deploy anywhere.** One binary, Docker, static files, or the edge.

## Performance

On a server-rendered HTML benchmark on Linux, Wisp serves 13% more requests than Actix Web and 27% more than Axum, in 3 MB of memory, with no `unsafe` code. See [bench](bench/README.md).

## Documentation

- [docs/](docs/design.md): the template language, client, APIs, embedding and deploying.
- [AGENTS.md](AGENTS.md): everything on one page, for coding agents and people in a hurry.
- [llms-full.txt](llms-full.txt): the same for LLMs.

## License

[MIT](LICENSE)
