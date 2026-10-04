# Edge bench

Wisp's wasm build against Hono, SvelteKit (adapter-node) and Next.js
(standalone) on the same runtime. Three routes, the same output in each:
`GET /` ("hello", text), `GET /list` (HTML, 50 escaped items), `GET /json`.
`apps/` has the four apps. Load: `oha`, 10 s, 64 connections, 3 s warmup, median
of 3. Not measured: Deno (not installed here).

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

req/s, p99 in ms. Hono's Node adapter answers a plain Response without
building a Request, which is why it leads on `/` and `/json` there.

Inside the wasm a request costs about 1 us (1.5 us before the app's parts were
borrowed from the host's bytes, the reply head was sent by number and a task
that ends in its first poll was never stored). Optimization level: `3` for every
host but Vercel and Netlify `--edge`, which get `s`; `z` was 40% slower and no
smaller than `s`, and `+simd128,+bulk-memory` measured no faster. `app.wasm` for
this bench: 508 KB (178 KB gzipped) at `3`, 427 KB (159 KB) at `s`; Cloudflare's
limit is 3 MB gzipped.

| Runtime | Framework | `/` | `/list` | `/json` |
|---|---|---|---|---|
| Node | Wisp | 77,327 (1.4) | 48,679 (2.2) | 75,678 (1.5) |
| Node | Hono | 83,717 (1.3) | 37,619 (2.8) | 80,523 (1.4) |
| Node | SvelteKit | 16,720 (6.9) | 5,773 (23.5) | 15,463 (7.7) |
| Node | Next.js | 3,443 (78) | 861 (87) | 3,306 (86) |
| workerd | Wisp | 1,735 (146) | 1,381 (196) | 1,692 (160) |
| workerd | Hono | 1,597 (155) | 1,602 (154) | 1,841 (173) |

`wrangler dev` caps both workerd rows near 1,500 to 2,000 req/s (its dev proxy
is the limit, and runs vary by 15%), so read them as a tie. Other machine load
moves the Node numbers by the same amount: run on a quiet machine.
