# Wisp

A fast, fun web framework for Rust: file routes, `.wisp` templates compiled to Rust, form actions, one binary.

Commands: `cargo build` · `cargo test -q` · `cargo fmt --check` · `cargo clippy --all-targets -- -D warnings` (also with `--target x86_64-unknown-linux-gnu`) · token bench `cargo run -q -p wisp-tokens --release`.

Crates: `wisp` runtime (I/O drivers, http, tables) · `wisp-build` build.rs codegen, templates, checks · `wisp-macros` derives and attributes · `wisp-shared` code shared by build and runtime · `wisp-cli` the `wisp` command (new, dev, build, mcp) · `tests/`, `examples/`, `bench/` apps, tests, benchmarks.

Hard rules, in order:
1. Ultra fast: zero cost on the request hot path, proven by an instructions-per-request A/B.
2. Cheap in tokens for app authors.
3. Durable: proven, self-tested at startup, falls back, never panics at runtime.
4. Flexible.

Carmack-style code, minimal deps, no `unsafe` (workspace lint), match the surrounding style. Never hand-edit `crates/wisp-cli/templates/vendor` (build.rs regenerates it). Never commit `todo.txt`.

Rules load by path from `.claude/rules/` (runtime, build, cli, tests-bench). Writing a Wisp app: skill `wisp-app`. `llms/AGENTS.md` is the framework reference for app authors and other tools (read it only for app work or when asked); `docs/design.md` is the contract.

Docs move with code: a change to the API, syntax, CLI or behavior updates, in the same change, the `///` docs, the LSP hover tables (`crates/wisp-cli/src/lsp.rs`), `llms/AGENTS.md`, `docs/*.md`, `examples/` and the vendored copies (rule `.claude/rules/docs-sync.md`).
