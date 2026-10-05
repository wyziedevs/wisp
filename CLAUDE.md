# Wisp

A fast, fun web framework for Rust: file routes, `.wisp` templates compiled to Rust, form actions, one binary.

Commands (all of them run on Windows, macOS and Linux; the whole gate set is `cargo run -q -p wisp-gate`, add `fast` to skip the wasm steps): `cargo build` · `cargo test -q` · `cargo fmt --check` · `cargo clippy --all-targets -- -D warnings` (also with `--target x86_64-unknown-linux-gnu`) · token bench `cargo run -q -p wisp-tokens --release`.

Crates: `wisp` runtime (I/O drivers, http, tables) · `wisp-build` build.rs codegen, templates, checks · `wisp-macros` derives and attributes · `wisp-shared` code shared by build and runtime · `wisp-cli` the `wisp` command (packages are published as `wisp-web` for the CLI and `wisp-web-rt`, `wisp-web-build`, `wisp-web-macros`, `wisp-web-shared`; lib names unchanged, so `-p wisp-web-rt` etc.) (new, dev, build, mcp) · `tests/`, `examples/`, `bench/` apps, tests, benchmarks.

Hard rules, in order:
1. Ultra fast: zero cost on the request hot path, proven by an instructions-per-request A/B.
2. Cheap in tokens for app authors.
3. Durable: proven, self-tested at startup, falls back, never panics at runtime.
4. Flexible.

Carmack-style code, minimal deps, no `unsafe` (workspace lint; only the Linux I/O drivers and the edge exports in `crates/wisp` are allowed), match the surrounding style. Never hand-edit `crates/wisp-cli/templates/vendor` (build.rs regenerates it). Never commit `todo.txt`.

Rules load by path from `.claude/rules/` (runtime, build, cli, tests-bench). Writing a Wisp app: skill `wisp-app`. `llms/AGENTS.md` is the framework reference for app authors and other tools (read it only for app work or when asked); the contract is https://wispweb.dev/docs/design (repo `../wisp-docs`).

Docs move with code: a change to the API, syntax, CLI or behavior updates, in the same change, the `///` docs, the LSP hover tables (`crates/wisp-cli/src/lsp.rs`), `llms/AGENTS.md`, `examples/` and the vendored copies (rule `.claude/rules/docs-sync.md`). Human docs live in the sibling repo `../wisp-docs` (`src/routes/docs/<slug>/+page.md`, site https://wispweb.dev, deploys from its `main`): update it in the same change.

Website rule (always): after ANY change to the framework (features, fixes, security hardening, behavior, speed work, new benchmark or token numbers), update https://wispweb.dev in the same change: the matching docs pages, the landing page and the speed/token/status numbers, then commit and push `../wisp-docs` `main` so the live site matches the code. A framework change is not done until the site is. Quote only measured numbers, with their source.
