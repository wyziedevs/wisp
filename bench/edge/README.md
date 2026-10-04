# Edge bench

Wisp's wasm build against Hono, SvelteKit (adapter-node) and Next.js
(standalone) on the same runtime. Three routes, the same output in each:
`GET /` ("hello", text), `GET /list` (HTML, 50 escaped items), `GET /json`.
`apps/` has the four apps. Load: `oha`, 10 s, 64 connections, 3 s warmup, median
of 3. Not measured: Bun and Deno (not installed here; their shims use the same raw driver).

Setup (from a bench dir, say `C:/wb`):

```sh
cargo install oha
wisp new app                                        # then replace app/src/routes with apps/wisp/src/routes, delete app/static
cd app && wisp build --target node --out wisp-node            # and --target cloudflare --out wisp-cf
mkdir hono sveltekit next                           # copy apps/*, then in each: npm i
(cd hono && npm i hono @hono/node-server wrangler)
(cd sveltekit && npm i && npx vite build)           # dir name is `sk` in run.mjs
(cd next && npm i && npx next build)
node run.mjs --dir <bench dir>
```

Directory names `run.mjs` expects: `wisp-node wisp-cf hono sk next`.

The Wisp app has no `static/` folder: with one, `wisp build --target
cloudflare` adds an `[assets]` binding, and `wrangler dev` pays about 30% per
request for the asset check before the worker runs.

## Results (Windows 10, 16 cores, Node 26, workerd via `wrangler dev --local`)

req/s, p99 in ms. Wisp's Node server reads raw sockets and the app's own HTTP parser
answers (no `node:http` objects per request), which is why it is above the
`node:http` floor; `WISP_NODE_HTTP=1` serves through `node:http` instead.
`run.mjs` has both (`wisp-node`, `wisp-node-http`) and the floor (`hello-node`,
`apps/hello`).

Inside the wasm a request costs about 1 us (1.5 us before the app's parts were
borrowed from the host's bytes, the reply head was sent by number and a task
that ends in its first poll was never stored). Optimization level: `3` for every
host but Vercel and Netlify `--edge`, which get `s`; `z` was 40% slower and no
smaller than `s`, and `+simd128,+bulk-memory` measured no faster. `app.wasm` for
this bench: 508 KB (178 KB gzipped) at `3`, 427 KB (159 KB) at `s`; Cloudflare's
limit is 3 MB gzipped.

| Runtime | Framework | `/` | `/list` | `/json` |
|---|---|---|---|---|
| Node | Wisp (raw sockets) | 127,810 (0.8) | 73,176 (1.6) | 128,582 (0.8) |
| Node | Wisp (`WISP_NODE_HTTP=1`) | 71,758 (1.8) | 45,876 (2.5) | 73,933 (1.5) |
| Node | Hono | 83,157 (1.3) | 38,076 (3.0) | 80,715 (1.4) |
| Node | node:http "hello" floor | 90,201 (1.2) | 90,508 (1.2) | 89,809 (1.2) |
| Node | SvelteKit | 16,720 (6.9) | 5,773 (23.5) | 15,463 (7.7) |
| Node | Next.js | 3,443 (78) | 861 (87) | 3,306 (86) |

Other machine load moves the Node numbers by about 15%: run on a quiet machine.

## workerd, without wrangler

`wrangler dev` caps every app near 1.7k req/s (its dev proxy is the limit), so
the Workers numbers come from the `workerd` binary itself (npm package
`workerd`, run as `workerd serve config.capnp`). `workerd.mjs` writes the
config (one worker, every `.js`/`.mjs`/`.wasm` of the folder as a module),
starts both apps, and alternates oha runs between them so a busy machine hurts
both alike. It reports req/s, p99 (ms), workerd's CPU microseconds per request
(process time over the requests served, so it includes workerd's own HTTP work;
a bare `new Response("hello")` worker costs about 45) and cold start (process
start to the first complete response, median of 15).

```sh
(cd hono && npx wrangler deploy --dry-run --outdir ../hono-out)   # bundles Hono; rename app.js to app.mjs
node workerd.mjs --workerd hono/node_modules/@cloudflare/workerd-windows-64/bin/workerd.exe \
  --wisp wisp-cf --hono hono-out [--secs 10 --runs 3 --cold 15 --only wisp]
```

Windows 10, 16 cores, c=64, 10 s, median of 3; req/s (p99 ms), CPU us/request:

| | `/` | `/list` | `/json` | cold start |
|---|---|---|---|---|
| Wisp before | 14,745 (32.5) 70 | 11,026 (35.4) 94 | 13,820 (31.3) 75 | 27 ms |
| Wisp now | 17,813 (5.1) 59 | 15,153 (6.2) 67 | 17,165 (5.2) 60 | 28 ms |
| Hono | 20,582 (4.5) 49 | 15,842 (7.9) 66 | 19,786 (27) 53 | 22 ms |

What moved Wisp (`bridge.js` `serve`): a request without a body goes to the app
synchronously and its Response is returned directly (no Promise, closure or
joined copy of the request), the request text's bytes are cached by the text,
and a response head becomes `new Response`'s init once. That cut p99 six-fold.
Measured and dropped: `headers.forEach` (no faster), a string body instead of
the Uint8Array (no faster). `strip` takes the wasm from 508 KB to 466 KB and
`opt-level=s` to 373 KB, with no change in cold start: about 3 ms is V8
compiling the module, and about 4 ms more is its first calls (`main`, then the
first request's code, compiled on first use).

Still behind Hono by 10 to 15% on `/` and `/json`: iterating the request's
headers (about 2 us in workerd) and the wasm call are what Hono does not do.
