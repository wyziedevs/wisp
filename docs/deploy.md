# Deploying

| You have | Use |
|---|---|
| VPS or server | `wisp build`, copy the binary |
| Container host (Fly.io, Railway, Render, Cloud Run, Azure Container Apps) | `wisp build --docker` |
| Static host (GitHub/GitLab Pages, S3) | `wisp build --static` (or `--spa`) |
| Edge or serverless (Cloudflare, Deno Deploy, Vercel, Netlify, Amplify, Firebase, Azure Static Web Apps) | `wisp build --target <host>` |
| AWS Lambda / Bun | `--target lambda` / `--target bun` |

An app that signs cookies needs `WISP_SECRET` (32+ random characters) on
every host.

A plain `wisp build` inside a host's CI picks that target from its
variables and says so: `WORKERS_CI` or `CF_PAGES` (cloudflare), `VERCEL`,
`NETLIFY`, `DENO_DEPLOYMENT_ID` (deno), `AWS_APP_ID` (node). Vercel gets
`.vercel/output` in the app folder. `--target native` forces the plain
binary.

`wisp deploy init <cloudflare|deno|vercel|netlify|lambda|fly|pages>` writes
`.github/workflows/deploy.yml` (build and deploy on push to `main`; its first
line names the secrets; `--force` replaces it). `wisp deploy init
fly|render|railway` writes that host's config (and a Dockerfile if none).

## Binary, static, prerender

`wisp build`: one release binary with static files and styles inside,
listening on `$HOST:$PORT` (`0.0.0.0:3000` in release).

`wisp build --static [--out site]` writes `dist/`: every parameterless page
as `about/index.html`, plus `static/` and the `/_app` files (with
`--sourcemap`, their `.map`s). A `[params]` route lists its pages:

```html
<!-- src/routes/blog/[slug]/+page.wisp (or its +page.rs) -->
---
fn entries() -> Vec<&'static str> {
    vec!["hello", "second-post"]
}
---
```

`entries` returns a `String` or `&str` per param, or a tuple in path order;
for `[[optional]]` and `[...rest]` an empty string leaves it out. A route
with actions or a `+server.rs` needs a server (the export warns).

`--spa` is `--static` plus an `index.html` fallback (Netlify:
`/* /index.html 200` in `_redirects`; Cloudflare Pages with no `404.html`).
A `const SSR: bool = false;` page whose `[params]` have no `entries` is
written once (params `0`) to `_app/spa/N.html`; `index.html` lists them and
wisp.js draws the one whose route fits, with that address's params. It
gets its data from `+page.js`.

Prerender in a server build:

```html
---
const PRERENDER: bool = true;
fn entries() -> Vec<&'static str> { vec!["hello", "second-post"] }  // with [params]
let post = db::post(&slug).await?;
---
```

`wisp build` builds the binary, runs it once (`init` runs) to render those
pages, builds again with the bytes inside, and serves them as they are (ETag,
304). Before that (`cargo run`, other targets) each worker keeps the first
render. One render serves all, so `cx` in statements, markup or `load` is a
build error; its layouts render once as for a request without cookies.
`--static` prerenders every page.

## Docker

```sh
wisp build --docker              # --force replaces existing files
docker build -t my-app .
docker run -p 3000:3000 -e WISP_SECRET=... my-app
```

A two-stage `Dockerfile` (`rust:slim` then `debian:stable-slim`, `HOST=0.0.0.0`,
`WISP_DATA=/data`) and `.dockerignore`. Saved tables live in `/data`: mount
a volume (`-v my-app-data:/data`). Docker's default seccomp refuses
io_uring, so the server uses an epoll per worker; a profile allowing
`io_uring_setup`, `io_uring_enter`, `io_uring_register` brings it back.

## Logs, metrics, traces

Off until set; binary, Docker, Lambda; no app code.

`WISP_LOG=json`: a JSON line per request on stdout (`off` default):
`{"time","method","route":"/blog/[slug]","path":"/blog/hello","status","ms","bytes","id","ip"}`.
`route` is the route folder (null if none), `path` omits the query, `ip` is
`cx.client_ip()` (`WISP_CLIENT_IP_HEADER`); the id is the client's
`x-request-id` or made, and is echoed.

`METRICS_KEY` serves `/_wisp/metrics` (Prometheus text) to
`Authorization: Bearer <METRICS_KEY>` (else 401; unset: 404; scrape config
`metrics_path: /_wisp/metrics`, `authorization: { credentials: <key> }`):
`wisp_requests_total{route, status}` (status class `2xx`; `route=""` if none
matched), `wisp_request_duration_seconds{route}` (histogram, 1 ms to 10 s),
`wisp_requests_in_flight`, `wisp_uptime_seconds`,
`process_resident_memory_bytes` (Linux).

`OTEL_EXPORTER_OTLP_ENDPOINT` (`http://localhost:4318`; or
`OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`, used as is) sends OTLP/HTTP JSON traces
to `<endpoint>/v1/traces`: a server span per request (`GET /blog/[slug]`),
an incoming `traceparent` continued, the reply carrying its own (`trace` in
`WISP_LOG=json`). `OTEL_SERVICE_NAME`, `OTEL_EXPORTER_OTLP_HEADERS`
(`api-key=…,x=…`), `OTEL_BSP_SCHEDULE_DELAY` (ms, 5000). A down collector
costs requests nothing. Plain HTTP only (use a Collector for TLS).

```rust
let _s = wisp::span("charge card"); // child span until dropped
let t = wisp::traceparent();        // Some("00-…-01"): header for a downstream call
```

## Edge and serverless: `--target`

```sh
rustup target add wasm32-unknown-unknown     # once
wisp build --target cloudflare               # dist/cloudflare (--out <folder>)
```

The app compiles to WebAssembly in a folder with the host's config, an entry
file and the static files; no wasm-bindgen or other tool.

| Target | For | From `dist/<target>` |
|---|---|---|
| `cloudflare` | Workers | `npx wrangler deploy` (secrets: `npx wrangler secret put WISP_SECRET`) |
| `pages` | Cloudflare Pages | `npx wrangler pages deploy .` in `dist/pages` (`_worker.js`, `_routes.json`; `WISP_SECRET` under Settings) |
| `deno` | Deno Deploy | `deployctl deploy --entrypoint main.ts` (local: `deno run -A main.ts`) |
| `vercel` | Vercel | `npx vercel deploy --prebuilt` (env var `WISP_SECRET`) |
| `netlify` | Netlify | `npx netlify deploy --prod` |
| `node` | Amplify, Firebase, Azure, Stormkit, Zeabur, any Node host | `npm start` |
| `bun` | Bun (`Bun.serve`) | `bun server.mjs` |
| `lambda` | AWS Lambda | below |

Vercel and Netlify Edge: add `--edge` (`--target vercel --edge`, `--target netlify --edge`); the wasm app runs as a module (`opt-level = "s"`, for their size limits), Netlify skips `static/` via `excludedPath`. Edge limits apply.

The `node` server reads at most `WISP_BODY_LIMIT` (default 1 MB) of a body
and answers 413 past it; raise it for a route with a larger `BODY_LIMIT`.

- **Amplify, Firebase, Azure Static Web Apps, Stormkit, Zeabur:** `--target
  node` (`npm start`); the output's `hosts/*.md` has each host's manifest or
  function glue (Amplify: folder in `.amplify-hosting/compute/default/`,
  `static/` in `.amplify-hosting/static/`, a `deploy-manifest.json`).
- **GitHub/GitLab Pages:** `--static`, publish `dist/` (a project site under
  a path prefix needs prefix-safe links).
- **Fly.io:** `--docker`, `fly launch`, `fly deploy`, `fly secrets set WISP_SECRET=...`.
  **Railway, Render:** `--docker`, point at the repo, set `WISP_SECRET`.
  **Cloud Run:** `--docker`, `gcloud run deploy --source .`.
- **AWS Lambda:** `rustup target add x86_64-unknown-linux-musl` once;
  `--target lambda` writes `dist/lambda/bootstrap.zip` (the app's own static
  binary via Rust's lld; no C toolchain). Create the function once (runtime
  `provided.al2023`, `x86_64`, handler `bootstrap`) and add a Function URL
  (or API Gateway/ALB):

```sh
aws lambda create-function --function-name my-app --runtime provided.al2023 \
  --architectures x86_64 --handler bootstrap --role <role-arn> \
  --zip-file fileb://dist/lambda/bootstrap.zip
aws lambda update-function-code --function-name my-app --zip-file fileb://dist/lambda/bootstrap.zip
```

Any Wisp binary answers Lambda's runtime API when `AWS_LAMBDA_RUNTIME_API`
is set. Everything works except WebSockets and streaming (a stream is sent
whole). Saved tables go in `/tmp`, per instance: use `wisp::store` for
lasting data. The `tower` feature with `lambda_http` ([embed.md](embed.md))
also works.

### What works on the edge

No threads, sockets or files:

- Use `wisp::spawn` and `wisp::sleep`, not tokio's (they map to the host's
  task queue and `setTimeout`). `tokio::spawn`, `tokio::time`, sqlx, reqwest
  and `Response::file_in` answer 500 there; all work in the binary, Docker,
  Lambda.
- Background work: Cloudflare and Netlify keep the instance alive
  (`waitUntil`) for started timers and fetches; Deno and Node run on;
  Vercel may freeze after the response, so finish first.
- Saved tables (`Rest`, `Table::saved`) are per-instance memory unless
  `WISP_STORE` is set (no app code):

  | `WISP_STORE` | Store |
  |---|---|
  | `d1:DB` | Cloudflare D1 binding `DB` (`[[d1_databases]]` in wrangler.toml) |
  | `deno-kv` | Deno KV (`deno-kv:<path>` file or URL) |
  | `libsql://name.turso.io` | Turso or any libSQL server over HTTP, with `WISP_STORE_TOKEN` |

  Each instance reads every row at start (tables must fit in memory); a
  request's changes are one batch before the answer; a failed batch answers
  500 and the next request gets a fresh instance. Rows: SQL table
  `wisp_rows (tbl, id, json)`, or `["wisp", table, id]` in Deno KV.
- `WISP_SECRET` as a host secret. Read others with `wisp::env("KEY")`
  (`std::env::var` sees nothing; no `.env`).
- Streaming (`Response::stream`, `Response::events`) is live on Cloudflare,
  Deno, Netlify, Vercel, Node; a leaving client fails `send`.
- `Response::websocket` is 501 on every edge target (and `tower`); use SSE.
- `wisp::channel`, `wisp::every`, `RateLimit` are not in the edge build (it
  won't compile with them): use the host's queues, cron, rate limiting.
- Outbound HTTP via `wisp::edge::fetch`:

  ```rust
  let mut req = wisp::Request::new("POST", "https://api.example.com/rows");
  req.header("authorization", &format!("Bearer {}", wisp::env("API_KEY").unwrap_or_default()));
  req.body = json.into_bytes();
  let reply = wisp::edge::fetch(req).await?;
  ```

- A panic fails only that request (500). No `Date` header from Wisp.
