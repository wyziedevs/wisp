---
paths:
  - "crates/wisp-build/**"
  - "crates/wisp-macros/**"
---
# Build and macros

- wisp-build runs in each app's `build.rs` and emits `$OUT_DIR/wisp.rs`: modules, one render fn per template, the router `match`, the `wisp::App` impl. Output is meant to be read; an error inside a template expression ends its line with `// file.wisp:line`.
- Templates compile to straight-line `push_str`; routes to one `match`; no boxing or dynamic dispatch on the hot path. Decide at build whatever can be decided (which routes never wait, baked pages, prop types).
- Mistakes fail early in the user's own file: check at build, say where and what to do. rustc should only point at code the user wrote.
- Escaping: holes never go in `on*` attrs, tag names, `javascript:` URLs, SVG animation values or `<meta http-equiv>`. Keep that check when touching template.rs.
- std only (plus `pulldown-cmark` for Markdown). No new deps without a reason in docs/design.md.
- Goldens (`tests/app/tests/golden.*`) pin generated output: a codegen change that alters it must be intended. `tests/agents` compiles the code blocks of AGENTS.md, so a syntax change updates AGENTS.md and docs/.
- Syntax or API change: update AGENTS.md (apps get it), docs/*.md, `wisp fmt` and the LSP (fmt.rs, ide.rs) and the editors/ grammar if affected.
- Test: `cargo test -q -p wisp-build -p wisp`, then the apps under `tests/`.
