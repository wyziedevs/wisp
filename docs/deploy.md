# Deploying

Wisp runs as one binary, but that is not the only way. Pick what your host
takes.

| You have | Use |
|---|---|
| A VPS or server | `wisp build`, copy the binary |
| A container host (Fly.io, Railway, Render, Cloud Run, Azure Container Apps) | `wisp build --docker` |
| A static host (GitHub Pages, GitLab Pages, S3) | `wisp build --static` (or `--spa`) |
| An edge or serverless host (Cloudflare, Deno Deploy, Vercel, Netlify, Amplify, Firebase, Azure Static Web Apps) | `wisp build --target <host>` |
| AWS Lambda | `wisp build --target lambda` |
| Bun | `wisp build --target bun` |

Every host needs `WISP_SECRET` (32 or more random characters) if the app
signs cookies.

## In a host's CI

A plain `wisp build` inside a host's build picks that host's target from the
variables it sets, and says so: `WORKERS_CI` or `CF_PAGES` (cloudflare),
`VERCEL`, `NETLIFY`, `DENO_DEPLOYMENT_ID` (deno), `AWS_APP_ID` (Amplify:
node). On Vercel it writes `.vercel/output` in the app's folder, where
Vercel looks. `wisp build --target native` builds the plain binary anyway.

## GitHub Actions

```sh
wisp deploy init cloudflare      # or deno, vercel, netlify, lambda, fly, pages
```

Writes `.github/workflows/deploy.yml`, which builds and deploys on every push
to `main`. Its first line names the secrets to set; `--force` replaces it.

## One binary

```sh
wisp build
```

Writes one release binary with the static files and styles inside. Copy it
and run it. It listens on `$HOST:$PORT` (`0.0.0.0:3000` in release builds).

## Static export

For sites with no actions and no server code:

```sh
wisp build --static              # writes dist/
wisp build --static --out site
```

Every page that takes no parameters is rendered and written as
`about/index.html`, along with `static/` and the `/_app` files (each
module with what it imports; with `--sourcemap`, with its `.map`).

A route with `[params]` says which pages to write:

```html
<!-- src/routes/blog/[slug]/+page.wisp (or its +page.rs) -->
---
fn entries() -> Vec<&'static str> {
    vec!["hello", "second-post"]
}
---
```

`entries` returns a `String` (or `&str`) per parameter, or a tuple of them
for several, in path order. For `[[optional]]` and `[...rest]`, an empty
string leaves it out. A route with actions or a `+server.rs` needs a server,
and the export warns about it.

`wisp build --spa` is `--static` plus the fallback a static host answers a
missing path with, `index.html` (Netlify: `/* /index.html 200` in
`_redirects`; Cloudflare Pages does it with no `404.html`). A page with
`const SSR: bool = false;` whose `[params]` have no `entries` is written
once, its parameters `0`, to `_app/spa/N.html`, and `index.html` lists
them: wisp.js draws the one whose route fits the address, with that
address's parameters. Such a page gets its data from `+page.js` (its
statements ran once, for `0`).

## Docker

```sh
wisp build --docker              # --force replaces existing files
docker build -t my-app .
docker run -p 3000:3000 -e WISP_SECRET=... my-app
```

Writes a two-stage `Dockerfile` (built on `rust:slim`, run on
`debian:stable-slim` with `HOST=0.0.0.0`) and a `.dockerignore`. Saved
tables (`#[derive(Rest)]`, `Table::saved`) are kept in `/data` in the
image: mount a volume there (`-v my-app-data:/data`) so they outlive the
container. Docker's default seccomp profile refuses io_uring, so there the
server runs on an epoll per worker rather than an io_uring (see docs/design.md);
a profile that allows `io_uring_setup`, `io_uring_enter` and
`io_uring_register` brings it back.

## Edge and serverless: `--target`

```sh
rustup target add wasm32-unknown-unknown     # once
wisp build --target cloudflare               # writes dist/cloudflare
```

The app is compiled to WebAssembly and written to a folder with the host's
config, a small entry file and the static files. There is no wasm-bindgen
and no other tool to install.

| Target | For | Deploy from `dist/<target>` |
|---|---|---|
| `cloudflare` | Cloudflare Workers and Pages | `npx wrangler deploy` |
| `deno` | Deno Deploy | `deployctl deploy --entrypoint main.ts` |
| `vercel` | Vercel | `npx vercel deploy --prebuilt` |
| `netlify` | Netlify | `npx netlify deploy --prod` |
| `node` | Amplify, Firebase, Azure, Stormkit, Zeabur, any Node host | `npm start` runs it |
| `bun` | Bun (with `Bun.serve`) | `bun server.mjs` runs it |

`--out <folder>` changes where it goes.

The `node` server reads at most `WISP_BODY_LIMIT` (1 MB by default) of a
request body and answers 413 past it; raise it for a route whose own
`BODY_LIMIT` is larger.

### Per host

**Cloudflare.** `wisp build --target cloudflare`, then `npx wrangler deploy`
in `dist/cloudflare`. Secrets: `npx wrangler secret put WISP_SECRET`.

**Deno Deploy.** `wisp build --target deno`, then `deployctl deploy
--entrypoint main.ts` in `dist/deno`. To try it locally: `deno run -A
main.ts`.

**Vercel.** `wisp build --target vercel`, then `npx vercel deploy --prebuilt`
in `dist/vercel`. Add `WISP_SECRET` under the project's environment variables.

**Netlify.** `wisp build --target netlify`, then `npx netlify deploy --prod`
in `dist/netlify`.

**AWS Amplify.** `wisp build --target node`. Put the folder in
`.amplify-hosting/compute/default/`, `static/` in `.amplify-hosting/static/`,
and write a `deploy-manifest.json`. `hosts/amplify.md` in the output has it
exactly.

**Firebase.** `wisp build --target node`. App Hosting runs `npm start`. For
Cloud Functions, wrap `server.mjs` with `onRequest`. See `hosts/firebase.md`.

**Azure Static Web Apps.** `wisp build --target node`. Managed functions
answer under `/api`, so an HTTP function passes every request to the app.
`hosts/azure.md` has the function and the `staticwebapp.config.json` rewrite.
App Service and Container Apps take the same folder (`npm start`) or a
Dockerfile.

**GitHub Pages and GitLab Pages.** `wisp build --static`, then publish
`dist/`. (A project site at `user.github.io/repo/` needs links that work under
a path prefix; a custom domain does not.)

**Fly.io.** `wisp build --docker`, then `fly launch` and `fly deploy`. Set
the secret with `fly secrets set WISP_SECRET=...`.

**Railway and Render.** `wisp build --docker`, then point the service at the
repository. Both build the Dockerfile. Set `WISP_SECRET` in the service's
variables.

**Stormkit and Zeabur.** `wisp build --target node`. Deploy the folder as a
Node.js service with start command `npm start`. Notes are in `hosts/`.

**AWS Lambda.** `rustup target add x86_64-unknown-linux-musl` once, then
`wisp build --target lambda` writes `dist/lambda/bootstrap.zip`: the app's
own binary, static and linked by Rust's lld, so no C toolchain is needed on
any machine. Create the function once with runtime `provided.al2023`,
architecture `x86_64` and handler `bootstrap`, and add a Function URL (or an
API Gateway or ALB in front):

```sh
aws lambda create-function --function-name my-app --runtime provided.al2023 \
  --architectures x86_64 --handler bootstrap --role <role-arn> \
  --zip-file fileb://dist/lambda/bootstrap.zip
aws lambda update-function-code --function-name my-app --zip-file fileb://dist/lambda/bootstrap.zip
```

Any Wisp binary answers Lambda's runtime API when `AWS_LAMBDA_RUNTIME_API` is
set; no app code. Everything the binary does works, but WebSockets and
streaming (a stream is sent whole). Saved tables go in `/tmp`, per instance:
use `wisp::store` for data that must last. The `tower` feature with
`lambda_http` ([embed.md](embed.md)) still works for an app of your own.

**Cloud Run.** `wisp build --docker`, then `gcloud run deploy --source .`.

### What works on the edge

The edge build has no threads, no sockets and no files, so a few things
differ.

- **Use `wisp::spawn` and `wisp::sleep`, not tokio's.** They are tokio's in
  the binary and the host's task queue and `setTimeout` on the edge. Code that
  needs the tokio runtime itself (`tokio::spawn`, `tokio::time`, sqlx,
  reqwest) answers 500 there, as does `Response::file_in`. All of it runs
  fine in the binary, Docker and Lambda.
- **Background work.** Cloudflare and Netlify keep a request's instance
  alive (`waitUntil`) until the timers and fetches it started are done. Deno
  and Node hosts keep running anyway. Vercel may freeze the function once the
  response ends, so finish the work before answering there.
- **Saved tables need `WISP_STORE`.** Without it, `#[derive(Rest)]` types
  and `Table::saved` are in memory, per instance. With it, they live in:

  | `WISP_STORE` | Store |
  |---|---|
  | `d1:DB` | Cloudflare D1, the binding `DB` (`[[d1_databases]]` in wrangler.toml) |
  | `deno-kv` | Deno KV (`deno-kv:<path>` for a file or a URL) |
  | `libsql://name.turso.io` | Turso or any libSQL server over HTTP, with `WISP_STORE_TOKEN`; any host |

  No app code. Each instance reads every row when it starts, so this is
  for tables that fit in memory, as on the binary. A request's changes are
  written in one batch before it is answered; a batch that fails answers
  500 and the next request gets a fresh instance, read from the store.
  Rows are in one SQL table, `wisp_rows (tbl, id, json)`, or under
  `["wisp", table, id]` in Deno KV.
- **Set `WISP_SECRET` as a host secret.** Read anything else with
  `wisp::env("KEY")`; `std::env::var` sees nothing on the edge. `.env` is
  not read there: the platform's variables and secrets are the env.
- **Streaming** (`Response::stream`, `Response::events`) is sent live on
  Cloudflare, Deno, Netlify, Vercel and Node, a chunk as it is made. A client
  that leaves makes the app's `send` fail, as on the binary.
- **No WebSockets.** `Response::websocket` answers 501 on every edge target
  (and under the `tower` feature). Use the binary or Docker for them, or
  server-sent events (`Response::events`), which work everywhere.
- **No in-process state across requests.** `wisp::channel`, `wisp::every`
  and `RateLimit` are not in the edge build (the app does not compile with
  them), since each request may run in an instance of its own: use the
  host's queues, cron triggers and rate limiting. JSON bodies, validation,
  CORS and the rest of [api.md](api.md) work everywhere.
- **Outbound HTTP** goes through `wisp::edge::fetch`, for a database over
  HTTP (D1, Turso, Supabase):

  ```rust
  let mut req = wisp::Request::new("POST", "https://api.example.com/rows");
  req.header("authorization", &format!("Bearer {}", wisp::env("API_KEY").unwrap_or_default()));
  req.body = json.into_bytes();
  let reply = wisp::edge::fetch(req).await?;
  ```

- **A panic** fails only that request, which answers 500.
- **No `Date` header** from Wisp; the host adds it.

If your app needs any of those, use Docker or the binary.
