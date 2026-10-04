---
name: wisp-app
description: Write or edit an app built with the Wisp Rust web framework (src/routes, .wisp templates, +page.wisp, +page.rs, +server.rs endpoints, #[action] forms, Table and #[model] data, Rest APIs, auth and members, components, hooks.rs, wisp dev/build/test). Use for app code, examples/, tests/app and bench apps; not for editing the framework crates.
---
# Writing a Wisp app

The full reference is `llms/AGENTS.md` (an app has its own copy at its root): read the section you need, not all of it. Deeper: `docs/client.md` (browser code), `docs/api.md` (Rest, OpenAPI), `docs/data.md` (tables, queues, cache), `docs/auth.md`, `docs/serve.md`, `docs/deploy.md`.

Core, in short:
- Files are routes: `src/routes/x/+page.wisp` (optional `---` Rust block, then markup), `+layout.wisp`, `+error.wisp`, `+server.rs` (fn get/post/...), `+page.rs` (`struct Data` + `fn load`). Models and tables live in `src/db.rs`; its `pub` items are in every route file. No `use` lines (there is a prelude).
- `#[action] fn name(args)` in a page block handles `<form action="?/name">`: params by name, `#[validate(..)]`, `redirect(..)`, `error(..)`, `invalid(..)`.
- Data: `#[model]` struct, `Table::saved()` (`new()` is in memory); `#[derive(Rest)]` is a whole JSON API.
- Auth: `cx.signup`, `cx.login`, `cx.user(&USERS)`.
- Fewest tokens wins: lean on conventions, don't write what the build infers. Run `wisp check`, `wisp test`, `wisp fmt` before finishing; `wisp routes` lists routes.
- Gotchas are in AGENTS.md (`Err(error(..))` is wrong, `+page.wisp` needs the `+`, no guards held across `.await`).
