# Docs move with code (always)

Any change that alters the public API, a template or Rust-block syntax, the CLI or behavior updates, in the same change:

- `///` on every `pub` item of `crates/wisp`: what it is, each argument, what it returns and when it errs, a tiny example where it helps. `#![deny(missing_docs)]` fails the build on a gap; rust-analyzer shows them on hover.
- The editor hover in `crates/wisp-cli/src/lsp.rs`: `KNOBS` (`const` knobs), `ATTRS` (`fields`, `action`, `use:enhance`, `data-wisp-*`), `DIRECTIVES`, `BLOCKS`. The test `hover_covers_the_reference` fails when `llms/AGENTS.md` or `docs/` shows a knob, `data-wisp-*` attribute or block it lacks.
- `llms/AGENTS.md` (the prelude list too), `docs/*.md`, `examples/` and anything else that names the thing.
- Then `cargo build -p wisp-cli` (refreshes `templates/vendor`), `cargo test -p wisp-cli` twice (the first run may regenerate `llms-full.txt`), and commit the vendored result.
