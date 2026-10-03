---
paths:
  - "crates/wisp/src/**"
---
# Runtime (crates/wisp/src)

- Hot path (http.rs, epoll.rs, uring.rs, swar.rs, cx.rs, headers.rs): zero added cost per request. Prove any change with an instructions-per-request A/B (`perf stat -e instructions`, c=64, GET and POST routes; callgrind for the split) against the parent commit. Keep only proven wins; drop cycle-noise "wins".
- No allocation after warm-up: buffers live per connection and are reused; idle connections hand pooled buffers back.
- Linux drivers: io_uring (6.1+), else an epoll per worker (`WISP_IO=epoll|uring`). Chosen by a startup self-test that runs the real code; one stderr line says which and why. Never pick a fast path by guessing; fall back, never crash at runtime.
- `unsafe` only in `uring.rs`, `epoll.rs` and the edge exports; the workspace lint forbids it elsewhere.
- `App::now`: routes the build proved never wait are answered on the driver (`http::on_driver`); the rest are handed to the connection future. An `async fn before` in hooks.rs takes every route off it, so keep `before` sync. Logs, metrics and traces keep the epoll fast path off, not a hook in it.
- Deps: only `tokio` and `httparse`. A new dependency needs a written reason in docs/design.md.
- Errors after startup return a response (`Error`), never `unwrap` or panic. Fuzz and robust-I/O tests (`fuzz.rs`, `tests/app/tests/robust_io.rs`) stay green.
- Behavior change: update docs/design.md (the contract), and AGENTS.md if app authors see it.
