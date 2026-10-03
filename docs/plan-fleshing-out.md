# Fleshing out: plan

Branch `fleshing-out`. Goal: everything a team expects from SvelteKit or Vue,
without giving up the design rule (1 fast, 2 cheap, 3 durable, 4 flexible).

Every feature below must pass these gates before it merges:

- **Fast:** zero cost for apps that don't use it. Request benchmarks stay flat
  (bench/), and dev rebuild time stays flat.
- **Cheap:** using it is one line, one file or one attribute. The compiler
  infers the rest.
- **Durable:** checked at build time with a clear error. Nothing new can panic
  at runtime, and every optional tool fails with a message, never a crash.
- **Done means:** tests, an AGENTS.md entry, a docs page section, and one
  commit series pushed.

## Phase 1: the compiler (everything else builds on it)

| # | Feature | How | App code |
|---|---|---|---|
| 1 | Scoped styles | A `<style>` in a page or component is scoped: each selector gets a `.w-<hash>` class, added to that file's elements at compile time. `:global(...)` opts out. The CSS is collected into `/_app/app.css` (no extra request). | `<style>h1 { color: red }</style>` |
| 5 | Source maps | The client codegen records a mapping for each script line back to its `.wisp` line, then writes v3 `.map` files in dev only (and in release with `--sourcemap`). | none |
| 3 | TypeScript in scripts | `<script lang="ts">`, `$lib/*.ts`, `+page.ts`. Types are stripped with our own erasable-syntax stripper in `js.rs` (the same rules as Node's `--strip-types`); enums and namespaces are build errors. No tsc and no swc. `wisp check --types` runs `tsc --noEmit` when Node is present, for full type checking. | `lang="ts"` |
| 14 | Accessibility warnings | Template lints run during `wisp check` and `wisp dev`: `img` without `alt`, `on:click` on a non-interactive element without a role or key handler, `label` without a control, empty `href`, `autofocus`, and so on. They are warnings; `<!-- wisp-ignore a11y-x -->` silences one. | none |
| S11 | Public and private environment variables | `wisp::env` stays server-only. `env.PUBLIC_X` in browser code is filled in at build time from `PUBLIC_*` variables. Any other name used in a script is a build error, so a secret can't reach the browser. | `env.PUBLIC_API_URL` |
| S9 | Content Security Policy | Pages already load scripts as modules. Add a nonce to every inline tag Wisp writes, and a default `content-security-policy` header with `self` plus the nonce. `wisp::csp("img-src https:")` in `init` extends it. Turn it off with `wisp::csp(None)`. | none |

## Phase 2: developer experience

| # | Feature | How |
|---|---|---|
| 0 | Editor support *(not in the list, but it matters most day to day)* | `wisp lsp`: a language server inside the CLI, reusing the parser and codegen to report diagnostics, hovers, go to definition (route, component, prop) and completion for props and directives. A small VS Code extension: a TextMate grammar (HTML + Rust in `---` and `{}` + JS in `<script>`) and a client that starts `wisp lsp`. |
| F | Formatter | `wisp fmt`: formats the markup in `.wisp` files, runs rustfmt on the `---` block and its own JS printer on scripts, and is idempotent. `wisp check` reports unformatted files. |
| 2 | Hot reload that keeps state | In dev, each component's client module gets a stable id. When a file changes, the dev socket sends the new module, and the runtime swaps the component's render and effects in place, keeping its `$state` values (matched by name) and the DOM state of inputs. A server-only template change morphs just that component's HTML, not the whole page. The current full swap stays as the fallback when a swap is not safe, e.g. the props changed. |
| 15 | Devtools | In dev, `Alt+Shift+W` opens an overlay, with no browser extension. It shows the component tree, live `$state` (editable), props, stores, the current route and params, the last action or navigation timings, and click-to-open in the editor. Built from `ui.css`. |
| 16 | Component workshop | `/_wisp/components` in dev lists every component. A `Card.stories.wisp` beside it holds named examples (`{#story "Featured"}<Card featured title="x" />{/story}`), shown with live props controls. No Storybook. |
| 11 | Browser E2E tests | `wisp test --browser` drives a local Chrome or Edge through the DevTools protocol (WebSocket, no Node, no Playwright). The test API is in Rust next to the HTTP tests: `let mut b = wisp::test::browser::<App>(); b.goto("/"); b.click("text=Plus one"); assert_eq!(b.text("output"), "1");`. When no browser is found, it skips with a message. |

## Phase 3: rendering and routing

| # | Feature | How | App code |
|---|---|---|---|
| R5 | Server rendering off, SPA mode | `const SSR: bool = false;` in a page's block sends that page's shell and data, and it renders in the browser. `wisp build --spa` writes `index.html` as a fallback for static hosts. | one const |
| R6 | Loading code on demand | `import('./x.js')` and `import('$lib/x.js')` resolve in scripts. Each page's client code is already its own module; shared code is split into chunks when 2+ pages import it. The import graph is emitted as `modulepreload` for the page's static imports only. | plain `import()` |
| R8 | Server functions | `#[remote] fn user(id: u64) -> Result<User>` in a page block or `src/remote.rs` becomes `await user(5)` in any script. It's a POST to `/_app/r/<hash>`, typed by the Rust signature (arguments and results as `#[derive(Json)]`), with the CSRF check and `before` hooks applied as for actions. `#[remote(get)]` makes it cacheable. | `await user(id)` |
| R7 | Server components | Already the default: a component with no `<script>` ships no JS. Gap to close: an island inside a server component inside an island. Make that work and document it as "server components" so people find it. | none |
| 12 | Shallow routing | `pushState(url, state)` and `replaceState` helpers. `page.state` is reactive; back and forward restore it with no server request. Used for modals and tabs. | `pushState('?tab=2', { tab: 2 })` |
| 13 | Snapshots | Form fields are captured automatically per history entry, and restored on back and forward or a reload. A page can export `snapshot = { capture, restore }` for its own state. | none, or one export |
| 8 | Per-page prerendering | `const PRERENDER: bool = true;` in a page's block (with `entries()` for params). `wisp build` renders it once to bytes, and the binary serves those bytes (ETag, immutable for the build). `--static` stays as "prerender everything". | one const |
| S13 | Trailing slash | `wisp::trailing_slash(Always | Never | Ignore)` in `init`, default `Never`. The other form 308-redirects, and links in templates are checked at build time. | one line |

## Phase 4: content and assets

| # | Feature | How | App code |
|---|---|---|---|
| 10 | Markdown pages | `+page.md` and `src/routes/blog/*.md`, with front matter (`title`, `layout`, any field becomes data) and components inside (`<Card title="x">markdown</Card>`). Rendered at build time with `pulldown-cmark` (one small dep), plus syntax highlighting at build time. A `[slug]` page can list them: `wisp::pages("blog")`. | write a `.md` file |
| 9 | Image optimization | `<img src="$lib/photo.jpg" alt="...">` (or `static/`) is noticed by the compiler. At build time it writes resized WebP plus the original format at 3 widths, with `srcset`, `sizes`, `width`, `height` and lazy loading. The `image` crate goes behind a CLI-only feature, so the runtime gets no new dep. In dev the original is served. | none |
| 7 | i18n | `src/locales/en.json`, `fr.json`. `{t("cart.items", count)}` in templates and `t('cart.items', count)` in scripts, with ICU-style plurals. Keys are checked at build time, so a missing key in any locale is an error. The `[[lang=locale]]` param, the `Accept-Language` header and a cookie pick the locale. Only the strings a page uses are sent to its JS. | `t("key")` |
| S12 | Sitemap and robots.txt | `/sitemap.xml` built from the route tree plus `entries()`, skipping params, `(private)` groups and `noindex` pages. `/robots.txt` points to it. A `static/` file of the same name overrides it. | none |

## Phase 5: platform and ecosystem

| # | Feature | How | App code |
|---|---|---|---|
| 6 | Service worker and PWA | `src/service-worker.js` is built with `import { build, files, version } from 'wisp/sw'` (lists of hashed assets) and registered for you. `src/manifest.json` (or `wisp::app_manifest(...)`) gives the web manifest plus icons sized from one `static/icon.png`. Opt-in `offline: true` precaches shell routes. | one file |
| W14 | Web components | `{@element "x-card"}` at the top of a component builds it as a custom element at `/_app/c/el/x-card.js`, with props as attributes and properties and slots as slots. It can be used on any site. | one line |
| 4 | Third-party UI | **A.** `wisp ui add button dialog tabs ...` copies accessible components (shadcn-style source the app owns, in Wisp's design tokens) into `src/components`. **B.** React, Vue and Svelte components from npm as islands: `<Island of="react:@radix-ui/..." client:visible props={...} />`, with the framework loaded only on that page via esm.sh. **C.** Web component libraries (Shoelace, Lit) work today; document them, add types. | `wisp ui add dialog` |
| S10 | Observability | `WISP_LOG=json` gives one structured line per request: method, route, status, ms, bytes and request id. `/_wisp/metrics` (Prometheus text, behind a bearer key) reports request counts and latency histograms per route. OpenTelemetry OTLP/HTTP export is turned on with `OTEL_EXPORTER_OTLP_ENDPOINT`. A feature off by default, so no cost when unset. | env vars |

## Order and size

1. **Phase 1, in order 1 → 5 → 3 → 14 → S11 → S9.** About two weeks of agent
   work. Unblocks the rest: source maps and TypeScript feed the editor
   support, and scoped styles feed hot reload.
2. **Phase 2:** 0 and F first (the largest daily win), then 2, 15, 16 and 11.
3. **Phase 3:** R8 and R6 first (the most asked for), then the rest.
4. **Phases 4 and 5** are mostly independent; they can run in parallel worktrees.

Each feature is one agent task in its own worktree off `fleshing-out`, with a
review and simplify pass before merging back. Benchmarks run on the Linux VPS
after each phase.

## Decisions

- **Images:** a pinned standalone `cwebp` (libwebp), downloaded to ~/.wisp/bin
  the way Tailwind and Sass are, with sha256 checked and a `$WISP_CWEBP`
  override.
  - It resizes and encodes lossy WebP. The `image` crate only writes lossless
    WebP, and it would add compile time.
  - If the tool can't be had, the build warns and serves the original image.
- **Markdown:** `pulldown-cmark`. It is CommonMark compliant and among the
  fastest parsers. It runs at build time only, so the runtime gets no dep.
- **E2E:** Rust only, next to the HTTP tests. One language means fewer tokens,
  and the same `wisp::test` API.
- **i18n:** lifted from the v0 non-goals. The framework supports translations.
  Wisp's own docs and messages stay English.
