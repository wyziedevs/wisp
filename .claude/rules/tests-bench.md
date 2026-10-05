---
paths:
  - "tests/**"
  - "bench/**"
---
# Tests and benchmarks

- `tests/gate` is the CI gate set (fmt, clippy -D warnings, tests, edge wasm clippy and size budget) as one cross-platform command: `cargo run -q -p wisp-gate`. No shell script may be a gate; `bench/**/*.sh` are Linux-box benchmark helpers only.
- `tests/app` is the main end-to-end app (http, forms, client, golden, robust_io, linux). `tests/agents` compiles AGENTS.md snippets. `tests/platform` and `tests/islands` cover targets and islands. Browser tests: `wisp test --browser`.
- Linux-only code (io_uring, epoll) is tested on a Linux machine over ssh, not guessed from Windows: upload with tar, run `cargo test -q` and the bench there; ask the user for the host. Also `cargo clippy --target x86_64-unknown-linux-gnu`. `git archive` keeps old mtimes: `touch` uploaded files or cargo will not rebuild.
- Speed claims: an instructions-per-request A/B (`perf stat -e instructions`, c=64, GET and POST) against the parent commit. Throughput is noisy (ranks swing 20% between runs, c=512 worst): never claim a win from one run; use `--rounds 6` or more, compare CPU per request, and say what stayed within noise.
- Windows cannot judge server changes: the load generator saturates first.
- `cargo run -r -p bench-run -- --only wisp,actix --rounds 6`; servers whose toolchain is missing are skipped.
- Token bench: `cargo run -q -p wisp-tokens --release` (tables on the tokens page of the docs site). A feature that makes app code longer is a regression.
