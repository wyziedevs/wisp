# Docs move with code (always)

Any change that alters the public API, a template or Rust-block syntax, the CLI or behavior updates, in the same change:

- `///` on every `pub` item of `crates/wisp`: what it is, each argument, what it returns and when it errs, a tiny example where it helps. `#![deny(missing_docs)]` fails the build on a gap; rust-analyzer shows them on hover.
- The editor hover in `crates/wisp-cli/src/lsp.rs`: `KNOBS` (`const` knobs), `ATTRS` (`fields`, `action`, `use:enhance`, `data-wisp-*`), `DIRECTIVES`, `BLOCKS`. The test `hover_covers_the_reference` fails when `llms/AGENTS.md` or the docs site pages show a knob, `data-wisp-*` attribute or block it lacks.
- The test `docs_cover_the_surface` (`crates/wisp-cli/src/coverage.rs`) fails naming each command, flag, knob, env var (classify new reads in its `ENV`), `wisp::`/`Cx`/`Response`/`Table` item, macro, `#[validate]` rule, built-in route, JS export or cargo feature the docs site lacks; reference tables: `/docs/cli`, `/docs/env`, `/docs/config`.
- The docs site repo `wisp-docs` (sibling checkout `../wisp-docs`, or `WISP_DOCS_DIR`): update the pages under `src/routes/docs/<slug>/+page.md` in the same change, commit there too. The site deploys from `main` of that repo, so push it with the Wisp change. Tests and llms-full.txt generation read those pages and skip cleanly when the checkout is absent.
- `llms/AGENTS.md` (the prelude list too), the `design` and `tokens` pages of the docs site, `examples/` and anything else that names the thing.
- Then `cargo build -p wisp-cli` (refreshes `templates/vendor`), `cargo test -p wisp-cli` twice (the first run may regenerate `llms-full.txt`), and commit the vendored result.
