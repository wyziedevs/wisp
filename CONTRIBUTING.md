# Contributing to Wisp

Wisp is MIT licensed and made by Wyzie LLC for the community. Issues and pull requests are welcome.

## Before you change code

Read [CLAUDE.md](CLAUDE.md). The rules, in order: ultra fast (zero cost on the request hot path, proven by an instructions-per-request A/B), cheap in tokens for app authors, durable (proven, self-tested at startup, falls back, never panics at runtime), flexible. No unsafe code outside the Linux I/O drivers and the edge exports, and few dependencies.

## Build and check

```sh
cargo build
cargo test -q
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo run -q -p wisp-gate fast     # the whole gate set, without the wasm steps
```

The same commands run on Windows, macOS and Linux.

## Docs move with code

A change to the API, syntax, CLI or behavior updates, in the same change, the `///` docs, the editor hover tables in `crates/wisp-cli/src/lsp.rs`, [llms/AGENTS.md](llms/AGENTS.md) and `examples/`. The human docs live in [wisp-docs](https://github.com/wyziedevs/wisp-docs) (`src/routes/docs/<slug>/+page.md`); send the matching pull request there. Never hand-edit `crates/wisp-cli/templates/vendor`: `cargo build -p wisp-web` regenerates it.

## Benchmarks

Benchmark output is data only: `results.json` and tables generated from it. A ranking sentence is derived by the generator, never written by hand, and a run that was not valid (for example one with heavy hypervisor steal) is not published.

## Pull requests

Keep a change small and say what it measures or proves. By contributing you agree that your work is released under the [MIT license](LICENSE). Be kind: see the [code of conduct](CODE_OF_CONDUCT.md). Security issues go to [SECURITY.md](SECURITY.md), not a public issue.
