# Edge bench

Wisp's wasm build against Hono, SvelteKit (adapter-node) and Next.js
(standalone) on the same runtime. Three routes, the same output in each:
`GET /` ("hello", text), `GET /list` (HTML, 50 escaped items), `GET /json`; and the
realistic routes at the end.
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
| Wisp sync | 17,813 (5.1) 59 | 15,153 (6.2) 67 | 17,165 (5.2) 60 | 28 ms |
| Wisp lazy headers | 20,294 (3.3) 50 | 16,665 (4.1) 62 | 20,693 (3.3) 50 | 27 ms |
| Hono (same run) | 21,112 (3.9) 49 | 16,877 (5.9) 62 | 20,826 (4.0) 49 | 23 ms |
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

Lazy headers: `serve` passes only `host`; the app asks the Request for
any other header when it reads it (`wisp_request_lazy`, imports `header` and
`headers`), so `/` and `/json` read none. Within 1 to 4% of Hono; the rest is
the first wasm entry of each request (about 3 us in workerd; later entries in
the same request cost 0.8 us). One difference from native: a request with
more than 100 headers is not refused with 431 on this path (a web `Headers`
has no count without iterating it, which is the cost removed; Cloudflare
refuses request heads over its own 128 KB limit).

The first wasm entry: an empty export (`wisp_current`) called once per
request costs as much as ten calls (about 2 to 4 us in workerd, 0.1 us in
Node), so it is workerd's per-request cost of entering wasm, not the app's.

Cold start: in Node, compile 1 ms (lazy), instantiate 0.1 ms, `main` 1 ms,
the first request's code compiled on first use most of the rest. Instantiating
synchronously on the first request measured no better in workerd (31 vs 37 ms,
noise about 6 ms), so it was dropped. `app.wasm` here: 511 KB (499 KB before).

## Realistic routes

`/list1000` (HTML, 1,000 escaped items), `/json-big` (200 objects of five
fields, one a list) and `/params/42?q=hello%20world&x=1` with `cookie: sid=abc123;
theme=dark` (a route param, a query value and a cookie read, answered as text).
The JSON and text are byte-identical in the two apps; the HTML differs in the
page head Wisp wraps around it (as `/list` always did), not in the items.
`workerd.mjs` and `run.mjs` take `--routes list1000,json-big,params` to run
only these. c=64; req/s (p99 ms), and workerd's CPU us/request. The machine
was shared (other processes move a result by 15 to 30%); the two apps
alternate within a run.

workerd, 8 s, median of 5:

| | `/list1000` | `/json-big` | `/params` |
|---|---|---|---|
| Wisp | 5,114 (27) 200 | 8,829 (11.7) 114 | 14,353 (10.6) 69 |
| Hono | 1,884 (63) 538 | 11,387 (9.7) 90 | 17,769 (28) 58 |

Node, 10 s, median of 3:

| | `/list1000` | `/json-big` | `/params` |
|---|---|---|---|
| Wisp (raw sockets) | 7,428 (14) | 18,295 (6.4) | 107,692 (1.1) |
| Hono | 1,776 (91) | 15,596 (17) | 49,016 (3.6) |

Wisp builds the big page 2.7 to 4 times as fast (the escape and the list are
one pass over bytes, Hono's are string work and a join). It is behind Hono
on workerd where the work is native there: `JSON.stringify` serializes the 200
objects in V8's C++ while Wisp's serializer runs in wasm (-22%), and a cookie
is a call out of the wasm into the Request's headers on top of the entry
(-19%). On Node, which has no entry cost, Wisp is ahead on all three.

## Cold start and wasm size, measured again

Cold start is process start to the first complete response, median of 9, three
rounds alternating the builds (the noise of one is about 6 ms; of a median of
9, 1 ms). Wisp 26 to 27 ms, Hono 21 to 23 ms, before and after every change
below. `app.wasm` of this bench's app (all routes above), `strip`ped, opt-level 3:

| | wasm | cold start |
|---|---|---|
| main | 527,151 | 27 ms |
| no dev-only files in the wasm32 build | 515,508 | 27 ms |

(Later main, with its newer features and the constant-path code below: 550,541 before, 540,872 after; 27 to 28 ms both.)

Kept: the dev reload script and the build-error dialog (`wisp-dev.js`,
`dialog.css`, 11.6 KB) are not linked on wasm32 (`http.rs`): the edge build has
no dev mode to serve them to. Measured and dropped:

- A request through a throwaway instance at module scope (compiles the request
  path before the first one): 45 ms, against 28. Instantiating twice costs more than
  the compiling it saves, and `env` (secrets, `WISP_STORE`) is not there yet to
  start the real instance.
- `obs` (log, metrics, traces), which never runs on wasm32, as a constant
  `None` there: 1.6 KB, and it changed the native `.text` (inlining of the
  functions around it), which has to stay as it is.
- `panic=abort`: already what wasm32-unknown-unknown builds (same bytes with it
  set); `lto = "fat"` and `codegen-units = 1` are in `wisp new`'s Cargo.toml.

Where the rest is (symbol sizes of the unstripped wasm; `cargo bloat` does not
read wasm, so the name section was read directly): 410 KB of code, 104 KB of data. Code:
alloc 62 KB, `wisp::edge` 56 KB, std 52 KB, core 50 KB, `wisp::http` 39 KB, then
`bake`, `cx`, `input`, `hashbrown`, `admin` (11 to 13 KB each). Data: 15 KB is
`wisp.js`, 6 KB the API docs page, 4 KB the error styles. There is no unused
subsystem left worth a flag: each remaining one is under 1% of the file, and
the edge build already leaves out the server's sockets, jobs and fetch.

## Paths that never change

A baked page (one the build proved constant, or prerendered) and a
trailing-slash redirect have one answer for every request, when the app has no
`before`, `after` or `reroute` hook, the route no guard (`RATE_LIMIT`, `CORS`,
`MIDDLEWARE`, `SIGNED_IN`, in the page or a layout), the app read no header but
`if-none-match` and `x-wisp-error`, and the request has no query. The app marks
the first answer `200 const` (its head's first line); `serve` in `bridge.js`
(the web `fetch` of Workers, Deno, Netlify, Vercel) keeps it, asks the app once
for that path's 304 (so the head is the app's own, not rebuilt by hand), and
answers GET, HEAD and `if-none-match` itself from then on, without entering the
wasm. Everything else goes to the wasm. `tests/platform/tests/fast.rs` compares
every header, the status and the body with the native server's for each case
(GET twice, HEAD, `if-none-match` with the ETag, `*`, a weak list, a wrong tag,
an empty one, `/about/`'s 308), and that an app with hooks, a query, a guard or
`x-wisp-error` never skips the wasm.

`/about` (a baked page) on workerd, c=64, same run, alternating: 13,101 req/s and
77 us CPU a request before, 16,387 and 64 after (best CPU of the runs: 75 and 54
us). Hono's `/about` is about the same as the new Wisp's (18,031 against 18,709
in a quiet run). The machine was shared; take the ratio, not the numbers.

Not covered, and why: Wisp's own files (`/_app/wisp.js`, the CSS) vary on
`accept-encoding` and `range`; `static/` is served by the host before the
worker; `CACHE` answers vary by cookie; the Node, Bun and Deno raw-socket path
hands the bytes to the app, which parses and answers in about 1 us, so a table
there would first have to parse HTTP in JS (not built, not measured).
