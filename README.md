<p align="center">
  <img src="examples/demo/static/favicon.svg" width="88" height="88" alt="Wisp">
</p>

<h1 align="center">Wisp</h1>

<p align="center">A fast, fun web framework for Rust.</p>

<p align="center">
  <a href="https://wispweb.dev/docs/">Docs</a> ·
  <a href="https://wispweb.dev/docs/quick-start/">Quick Start</a> ·
  <a href="https://wispweb.dev/docs/tutorial/">Tutorial</a> ·
  <a href="https://wispweb.dev/docs/benchmarks/">Benchmarks</a> ·
  <a href="https://wispweb.dev/blog/">Blog</a> ·
  <a href="https://discord.gg/2mxraHBVtB">Discord</a>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT license"></a>
  <img src="https://img.shields.io/badge/rust-1.88%2B-orange" alt="Rust 1.88 or later">
</p>

File routes, `.wisp` templates compiled to Rust, form actions that work without JavaScript, and one binary to deploy. A folder is a URL, and its `+page.wisp` is the page: a short block of Rust that loads data and handles forms, then markup. The whole app, styles and static files included, becomes one small binary.

## Quick Start

You need [Rust](https://rustup.rs) 1.88 or later.

```sh
cargo install wisp-web
wisp new my-app
cd my-app
wisp dev
```

Then open http://127.0.0.1:3000. The [Quick Start](https://wispweb.dev/docs/quick-start/) walks through the first page, and the [Tutorial](https://wispweb.dev/docs/tutorial/) builds a small app with a form and a table.

## Example

```html
<!-- src/routes/+page.wisp -->
---
let name = cx.query_or("name", "world".to_string());
---

<h1>Hello, {name}!</h1>
```

The `---` block is Rust that runs for each request, and `{name}` is rendered on the server. Turn JavaScript off and the page still works. The block also takes `#[action]` form handlers and a `mod server { … }` of JSON endpoints, so a whole route can be one file.

## Why Wisp

- **Fast.** A route pays only for the features it uses, and a change that touches the request path is checked by an instructions-per-request A/B before it lands.
- **Cheap to write.** The small test app takes 476 tokens in Wisp against 1,043 in Nuxt, 2.2x to 3.9x across the stacks measured. See [Tokens](https://wispweb.dev/docs/tokens/) for the method and the apps measured.
- **Durable.** Fast paths are proven at startup and fall back, and a panic in a handler is caught and answered as a 500. There is no unsafe code outside the Linux I/O drivers and the edge exports.
- **Flexible.** Forms, JSON APIs, uploads, signed cookies, hooks and components. Wisp gives you tools, not an auth or database layer.

## What You Get

- **Pages and forms:** `#[action]` handlers with validation, `use:enhance` for in-page updates, `#[derive(Rest)]` for a JSON CRUD API, saved tables in log files or any database.
- **Browser code:** `on:click="count++"` and `{:count}` with the state in the `---` block or none at all; `$derived` and `$effect` in a plain `<script>`, islands that load when needed, and a router that morphs pages.
- **Styling:** scoped CSS in a `<style>` block, Tailwind and Sass built in.
- **Tooling:** `wisp dev` with hot reload, `wisp fmt`, `wisp check`, `wisp test`, a language server with a VS Code extension, and `wisp mcp` to serve the docs to coding agents.
- **Hosting:** one binary, Docker, static HTML, or a build for Cloudflare, Deno Deploy, Vercel, Netlify, AWS Lambda, Bun and Node. [Pick your host](https://wispweb.dev/docs/hosting/).

## Speed

TechEmpower's plaintext and JSON tests, run with their own load scripts against their reference sources (SvelteKit and Next.js are plain route handlers), on one shared 4-vCPU AMD EPYC 7B13 VM with the server pinned to 2 cores. Medians of 3 runs of 15 seconds, 2026-10-04, from [`bench/tfb/results.json`](bench/tfb/results.json). Contenders are in a fixed order, not ranked; "Failed" means no run completed a request.

| Framework | Plaintext, 256 connections (req/s) | JSON, 64 connections (req/s) |
|---|---:|---:|
| **Wisp** | **1,129,577** | **96,089** |
| Axum | 276,948 | 78,427 |
| Actix Web | 603,163 | 89,648 |
| Express | 40,421 | 14,746 |
| Fastify | 51,713 | 21,965 |
| Hono (Node) | 28,966 | 8,988 |
| Hono (Bun) | 10,599 | 59,619 |
| SvelteKit | 10,929 | 10,378 |
| Next.js | 2,460 | 1,611 |

Rows whose min-max ranges overlap are ties. Plaintext, 256: Fastify and Express; Hono (Node), Hono (Bun), SvelteKit and Next.js (one SvelteKit run, measured before the harness drained the warmup backlog, reaches down to Next.js; re-measure pending). Next.js measured 2026-10-06 with that drain. JSON, 64: Wisp, Actix Web and Axum; Hono (Node) and SvelteKit.

This is not an official TechEmpower result, and the VM is shared. [`bench/tfb/RESULTS.md`](bench/tfb/RESULTS.md) has every contender, connection level and metric. Rank tables from a newer run are pending: the latest runs on a CPU-capped VPS were invalid and are not published. Valid on any machine: callgrind counts of 1585, 2323 and 1789 instructions per request for `GET /`, `GET /user/0` and `POST /user`, measured after the chunked-encoding fix and not rerun since later runtime commits (cddf6ca and on), so they may have moved.

## Docs

- [Design and Files](https://wispweb.dev/docs/design/): the template language, routing, actions and the runtime.
- [Browser Code](https://wispweb.dev/docs/client/): scripts, directives, islands, stores and the router.
- [APIs](https://wispweb.dev/docs/api/): JSON APIs, validation, auth, OpenAPI and testing.
- [Hosting](https://wispweb.dev/docs/hosting/): a page for every host.
- [CLI](https://wispweb.dev/docs/cli/), [Environment Variables](https://wispweb.dev/docs/env/) and [Knobs and Settings](https://wispweb.dev/docs/config/): the reference tables.
- [examples](examples): a demo app, a JSON API, and Wisp inside axum.
- [llms/AGENTS.md](llms/AGENTS.md): the whole reference for AI coding agents.

## Contributing

Issues and pull requests are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) and the design rules in [CLAUDE.md](CLAUDE.md) first: speed, then tokens, then durability, then flexibility. Security reports go through [SECURITY.md](SECURITY.md). The docs live in the [wisp-docs](https://github.com/wyziedevs/wisp-docs) repo and move with the code.

## License

[MIT](LICENSE). Open source, made by [Wyzie LLC](https://wyzie.io) for the community.
