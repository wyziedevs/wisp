<p align="center">
  <img src="examples/demo/static/favicon.svg" width="88" height="88" alt="">
</p>

<h1 align="center">Wisp</h1>

<p align="center">A fast, fun web framework for Rust.</p>

<p align="center">
  <a href="docs/design.md">Docs</a> ·
  <a href="#quick-start">Quick start</a> ·
  <a href="examples">Examples</a> ·
  <a href="llms/AGENTS.md">AGENTS.md</a>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT license"></a>
</p>

Wisp is the framework for the AI age: ultra fast to run, cheap in AI tokens to write, durable, and flexible. A folder is a URL, and its `+page.wisp` is the page: a short block of Rust that loads data and handles forms, then markup. Templates compile to plain Rust, and the whole app, styles and static files included, becomes one small binary.

## Quick start

You need [Rust](https://rustup.rs) 1.88 or later.

```sh
cargo install --git https://github.com/wyziedevs/wisp wisp-cli
wisp new my-app
cd my-app
wisp dev
```

Then open http://127.0.0.1:3000.

## Example

```html
<!-- src/routes/+page.wisp -->
---
let name: String = cx.query_or("name", "world");
---
<h1>Hello, {name}!</h1>

<button on:click="count++">Clicked {:count} times</button>

<script>
  let count = $state(0)
</script>
```

The `---` block is Rust that runs for each request, `{name}` is rendered on the server, and `{:count}` is JavaScript state in the browser. Turn JavaScript off and the server's HTML still works.

## Features

**Reactivity**
- `$state`, `$derived` and `$effect` in a plain `<script>`, with no build step.
- Islands (`client:visible`, `client:idle`, `client:media`) load code only when needed.
- Components are server-rendered and ship no JavaScript by default.
- Hot reload keeps your `$state`; markup edits show in under 100 ms.

**Built for AI and tokens**
- [AGENTS.md](llms/AGENTS.md) and [llms-full.txt](llms/llms-full.txt) hold the whole reference.
- `wisp mcp` serves the docs to coding agents.
- A whole app takes about half the tokens of SvelteKit or Next.js ([docs/tokens.md](docs/tokens.md)).
- The compiler infers types, so apps write fewer of them.

**Rendering**
- Server-side rendering, with streamed responses and server-streamed `{#await}` blocks.
- Prerendered pages in a server build, `wisp build --static` and `--spa`.

**Data and forms**
- `#[action]` form handlers with validation, and uploads.
- `#[remote]` functions called from the browser.
- `#[derive(Rest)]` gives a JSON CRUD API; a store keeps rows in log files or any database.

**Styling**
- Scoped CSS in a `<style>` block, Tailwind and Sass built in.

**Tooling**
- A language server (`wisp lsp`) with a VS Code extension, plus Zed, tree-sitter and Prettier.
- `wisp fmt`, `wisp check`, `wisp test` and `wisp test --browser`.
- Devtools on `Alt+Shift+W` and a component workshop in dev.

**Deploy**
- One binary, `--docker`, `--static`, or `--target cloudflare|deno|vercel|netlify|node|bun|lambda`.
- `wisp deploy init <host>` writes a GitHub Actions workflow or a Fly, Render or Railway config.

## Performance

On a server-rendered HTML page (the TechEmpower fortunes test without the database), on a 4-vCPU Linux VPS with 64 connections, from [bench](bench/README.md) (2026-09-28):

| Server | req/s | CPU µs/req | Peak MB |
|---|---:|---:|---:|
| **Wisp** | 97,502 | 19.8 | 3 |
| Actix Web | 86,169 | 23.0 | 5 |
| Axum | 76,818 | 25.6 | 6 |
| Fastify | 24,576 | 82.0 | 248 |
| SvelteKit | 2,676 | 758.5 | 494 |

Wisp has no `unsafe` code outside its Linux I/O drivers and the edge exports. The bench README has the method and the full results.

## Docs and links

- [docs/design.md](docs/design.md): the template language, routing, actions and the runtime.
- [docs/client.md](docs/client.md): scripts, directives, islands, stores and the router.
- [docs/api.md](docs/api.md): JSON APIs, validation, auth, OpenAPI and testing.
- [docs/deploy.md](docs/deploy.md): every host. [docs/embed.md](docs/embed.md): axum, hyper and Lambda.
- [examples](examples): a demo app, a JSON API and Wisp inside axum.

## Contributing

Issues and pull requests are welcome; read the design rule at the top of [AGENTS.md](llms/AGENTS.md) first.

## License

[MIT](LICENSE)
