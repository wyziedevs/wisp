# Wisp

A fast, fun web framework for Rust.

File routes, `.wisp` templates compiled to Rust, form actions that work without JavaScript, one binary.

```sh
cargo install --git https://github.com/wyziedevs/wisp wisp-cli
wisp new my-app
cd my-app
wisp dev
```

```html
<!-- src/routes/+page.wisp -->
---
let name: String = cx.query_or("name", "world");
---
<h1>Hello, {name}!</h1>
```

Docs, guides and benchmarks: https://wispweb.dev

MIT license. AI agents: [llms/AGENTS.md](llms/AGENTS.md).
