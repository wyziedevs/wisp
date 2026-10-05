# Releasing Wisp

`wisp` and `wisp-cli` are taken on crates.io (other projects), so Wisp
publishes as `wisp-web`:

| Package | Folder | Lib name | What |
|---|---|---|---|
| `wisp-web-shared` | crates/wisp-shared | `wisp_shared` | rules shared by build and runtime |
| `wisp-web-macros` | crates/wisp-macros | `wisp_macros` | derives and attributes |
| `wisp-web-rt` | crates/wisp | `wisp` | the runtime apps depend on |
| `wisp-web-build` | crates/wisp-build | `wisp_build` | build.rs codegen |
| `wisp-web` | crates/wisp-cli | (bin `wisp`) | the `wisp` command |

Apps keep the dependency keys `wisp` and `wisp-build` (`package = "wisp-web-rt"`),
so `use wisp::..` never changes. `crates/compat/{wisp,wisp-build}` (never
published) keep older apps with `wisp = { git = .. }` building.

## First publish (in order)

1. Check, on a clean `main`:
   ```sh
   cargo fmt --check && cargo clippy --all-targets -- -D warnings
   cargo test -q --workspace
   cargo package -p wisp-web-shared -p wisp-web-macros -p wisp-web-rt -p wisp-web-build -p wisp-web
   cargo publish --workspace --dry-run    # packages and builds all five against each other
   ```
   Each `.crate` is under 1.6 MB (`target/package/*.crate`).
2. Log in (you, once): `cargo login` with a token from https://crates.io/settings/tokens
   (scope: publish-new, publish-update).
3. Publish, dependencies first. Cargo waits for each to reach the index:
   ```sh
   cargo publish -p wisp-web-shared
   cargo publish -p wisp-web-macros
   cargo publish -p wisp-web-rt
   cargo publish -p wisp-web-build
   cargo publish -p wisp-web
   ```
   (`cargo publish --workspace` does the same in one command.)
4. Tag; the release workflow builds and uploads binaries to a draft release:
   ```sh
   git tag v0.1.0 && git push origin v0.1.0
   ```
   Check the draft on GitHub (5 archives, `.sha256` each, `SHA256SUMS`), then publish it.
5. Switch new apps to crates.io: in `crates/wisp-cli/src/dep.rs` set
   `pub const WISP_DEP: Dep = Dep::Crates("0.1");`, run `cargo test -p wisp-web`
   twice, commit, and release 0.1.1 the same way (steps 1, 3, 4).
6. Try it: `cargo install wisp-web`, `cargo binstall wisp-web`, `wisp new x`, `cd x && wisp dev`.
7. Optional: fill hashes in `packaging/` (Homebrew tap, Scoop bucket, winget PR)
   and host `scripts/install.sh` / `install.ps1` at wispweb.dev/install.

Later releases: bump `[workspace.package] version` and the `version =` of the
four `[workspace.dependencies]`, then steps 1, 3, 4.

## Irreversible

- A published version can never be deleted or overwritten, only yanked
  (`cargo yank -p wisp-web --version 0.1.0`); its source stays public.
- Crate names are yours once published; there is no unpublish.
- A pushed tag `v*` runs the release workflow at once.

## Optional: reserve the names now

Your call. Publish a 0.0.1 placeholder of each of the five names (a stub
`lib.rs`/`main.rs` and the description "Reserved for Wisp, https://wispweb.dev").
It holds the names; it is also public and irreversible like any version.

## Site and README changes at publish time (not before)

- README.md and the wispweb.dev home page and `/docs` getting-started:
  `cargo install --git https://wisp.ar0.eu wisp-web` becomes `cargo install wisp-web`
  (alternatives: `cargo binstall wisp-web`, `curl -fsSL https://wispweb.dev/install | sh`,
  `irm https://wispweb.dev/install.ps1 | iex`).
- Docs pages that show an app's Cargo.toml: `wisp = { version = "0.1", package = "wisp-web-rt" }`,
  `wisp-build = { version = "0.1", package = "wisp-web-build" }`.
- `/docs/cli` (update hint), `llms/AGENTS.md` line on updating the CLI,
  `examples/demo` about page, and `cargo test -p` names anywhere they appear.
- Docs pages naming cargo features keep `wisp/<feature>` (the key is still `wisp`).
