# Wisp

A fast, fun web framework for Rust.

File routes, `.wisp` templates compiled to Rust, form actions that work without JavaScript, one binary.

```sh
cargo install --git https://wisp.ar0.eu wisp-cli
wisp new my-app
cd my-app
wisp dev
```

```html
<!-- src/routes/+page.wisp -->
---
let name = cx.query_or("name", "world".to_string());
---
<h1>Hello, {name}!</h1>
```

Docs, guides and benchmarks: https://wispweb.dev

Made by [Wyzie LLC](https://wyzie.io). MIT license. AI agents: [llms/AGENTS.md](llms/AGENTS.md).
