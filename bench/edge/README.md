# Edge bench

Wisp's wasm build against Hono, SvelteKit (adapter-node) and Next.js
(standalone) on the same runtime. Three routes, the same output in each:
`GET /` ("hello", text), `GET /list` (HTML, 50 escaped items), `GET /json`; and the
realistic routes at the end.
`apps/` has the four apps. Ranking against the other popular frameworks on every host: see the end of this file and `../rank/`. Load: `oha`, 10 s, 64 connections, 3 s warmup, median
of 3. Deno and Bun are measured at the end of this file.

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

## Where json-big and params lose on workerd (measured)

Windows workerd runs the same wasm about 1.8x slower than Node (`/jb-ser`, the
200 rows' JSON of a prebuilt list: 19 us in Node, about 40 us over `/` in
workerd; building the rows with `format!`, 27 us in Node), and that does not
change over 25 s, so it is not tiering. `/json-big` is the sum of the two
(147 us against 116 for build alone and 107 for serialize alone, `/` 65): there
is no extra cost at the boundary, the body copy included. The gap to Hono there
is wasm compute (malloc, `fmt`, the serializer), not the bridge.

Kept (`bridge.js`, the `header` import): a header's name is read byte by byte
(ASCII) instead of through a `TextDecoder`, and its value is encoded straight
into the app's buffer instead of into a new array first. In Node, `serve` with
a cookie, fastest of 10 runs: 8.9 to 7.5 us. On workerd the 1 us is inside the
noise of a shared machine (15%), so no req/s claim is made.

Profiled again (Node, names kept, `/json-big`): the app's own `format!`
(`user-{i}`, `t{}`) is about 30% (`fmt::write`, `Display`, `pad_integral`),
dlmalloc and dropping the rows 20%, `live::string` 13%. Kept: in wasm,
`decimal` pushes its ASCII digits instead of calling `from_utf8` (3% of the
profile): `serve` fastest of 40 x 2000 calls, 36.2 to 33.9 us; `/` and
`/params` unchanged (1.0, 1.3 us). Native is unchanged (`cfg`). Tried, no gain:
`+simd128`; reusing the reply body across requests on edge (36.4 to 36.7 us).
Not possible: a bump or arena allocator, since `GlobalAlloc` needs `unsafe`.
Cold start in Node: compile 0.9 ms, instantiate 0.05 ms, first request 5 ms
(lazy compile of the big `poll`/`request` functions, about 3 ms); `wasm-opt` is
not installed here, so not tried. workerd A/B was inside the noise (2x swings).

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

## Clean run, 2026-10-04 (main 8642ce1, all runtimes)

Machine: Windows 10 Home 19045, AMD Ryzen 7 7800X3D (8 cores, 16 threads). Node 26.1.0,
Deno 2.5.2, workerd 1.20261001.1 (the binary under wrangler 4.147.0), Hono 4.13.12,
@hono/node-server 2.1.3, oha 1.16.0. Bun is not installed on this machine (no binary
found; not downloaded; measured later, see Bun below). Bench app: `apps/wisp` built by this
tree's `wisp` for each target; `apps/hono`. c=64, 10 s runs, 3 s warmup, **median of 5**,
cold start median of 9; Wisp and Hono alternate within every route (`ab.mjs` for Node and
Deno, `workerd.mjs` for workerd). The machine was not idle: an unrelated app held about
0.8 of a core throughout, which is why absolute numbers are half of the older tables
above; only the Wisp/Hono ratio inside a run means anything. No run had a non-200.

`wrangler dev --local` itself serves Hono `/params` at 584 req/s (p99 349 ms; one 5 s run):
its dev proxy is the limit, as above, so the workerd rows run the same `workerd` binary directly.

req/s (higher is better), Wisp / Hono:

| Runtime | `/` | `/list1000` | `/json-big` | `/params` + cookie | cold start (ms) |
|---|---|---|---|---|---|
| workerd | 7,694 / 9,447 | 5,093 / 2,007 | 5,962 / 8,480 | 14,581 / 14,624 | 39 / 24 |
| Node, raw sockets | 100,865 / 54,327 | 6,561 / 1,873 | 20,812 / 19,677 | 113,240 / 46,760 | 97 / 83 |
| Node, `node:http` (`WISP_NODE_HTTP=1`) | 46,129 / 54,327 | 6,084 / 1,873 | 16,850 / 19,677 | 52,128 / 46,760 | 72 / 83 |
| Deno, raw sockets | 118,877 / 92,507 | 6,426 / 2,022 | 9,027 / 5,702 | 93,841 / 64,674 | 64 / 51 |
| Deno, `Deno.serve` | 72,972 / 92,507 | 3,875 / 2,022 | 5,869 / 5,702 | 50,809 / 64,674 | 55 / 51 |
| Bun, raw sockets | 133,387 / 127,725 | 9,287 / 3,149 | 20,017 / 19,549 | 53,876 / 36,407 | 33 / 34 |
| Bun, `Bun.serve` (`WISP_NODE_HTTP=1`) | 83,345 / 127,725 | 8,521 / 3,149 | 18,703 / 19,549 | 40,962 / 36,407 | 36 / 34 |

workerd's `/` row was a separate 5 x 10 s run after the other three (the `workerd.mjs`
route filter swallowed `/` the first time), both apps in it alternating; its cold start
repeated at 39 / 27. workerd CPU us/request, Wisp / Hono (median): `/` 106 / 94,
`/list1000` 198 / 496, `/json-big` 147 / 117, `/params` 70 / 70. The Deno rows are a
second full run, after the fix below; the first run (quieter) gave Hono 108,071 on `/`
and 76,203 on `/params`, and Wisp raw 87,428 and 81,262, so the raw path was behind Hono
on `/` before the fix.

### Found by this run: Deno's raw path paid a timer per request

`deno.ts` re-armed its idle timeout (`clearTimeout` + `setTimeout`) on every read.
Measured with no Wisp at all (`Deno.listen` answering a fixed reply: 196k req/s; `Deno.serve`
`new Response('hello')`: 115k req/s), the loop has room, and the timer was 3 us a request:
Wisp raw `/` 87,347 -> 120,859 and `/params` 82,937 -> 113,014 (alternating, median of 5,
6 s) with one `setInterval` per connection that compares a timestamp stamped by each read
(idle close within 1.25x of 60 s). Kept (`crates/wisp-cli/src/targets/deno.ts`).

### WebSockets on workerd

`examples/websocket` built for cloudflare, run on the workerd binary, a Node `WebSocket`
client: the handshake and the numbered echo worked, but a client close was never answered
(the app had nothing to send, and workerd without `web_socket_auto_reply_to_close` waits
for the server end to `close`), so workerd cancelled the request ("code had hung") and the
client saw an error (1006) instead of 1000. `takeover` in `bridge.js` now closes the host's
socket when the client's close arrives (after telling the app, whose own close wins; the
code is the client's, or 1000 for 1005/1006). Verified on workerd: `1: hello`, `2: world`, a
70,003-byte echo, clean close 1000; `tests/app/tests/parity.rs` checks the reply.

### Where Wisp loses, and why (measured)

- **workerd `/`: -19%** (7,694 vs 9,447). In-process (Node, fastest of 40 x 2000 `fetch`
  calls) Wisp's `/` is 5.18 us against 3.06 us for the same `Request` and a `Response('hello')`
  with no wasm: 2.1 us above the floor (wasm entry, the request text, `Response` with the
  head); in Deno 3.62 vs 1.24 us (2.4 us). workerd runs the same wasm about 1.8x slower
  than Node and its first wasm entry in a request costs 2 to 4 us (above), so about 6 to 7 us
  more than a handler that is plain JS. Measured CPU is 106 us against 94 (best of runs 80 vs 64): the
  gap is inside the 15% this shared machine moves a result by; the quiet-machine
  table above had it at 1 to 4%.
- **workerd `/json-big`: -30%** (5,962 vs 8,480; CPU 147 vs 117 us, i.e. 41 us above Wisp's
  `/` against 23 us above Hono's `/`). Hono serializes 200 rows in `JSON.stringify` (V8's
  C++); Wisp's serializer is wasm (Node: 47.7 us a request, 42 of them above `/`; workerd
  x1.8). The kept escape fast path (16-byte then 8-byte word scans, clean runs copied in
  bulk) cannot help: the strings here are 1 to 8 bytes (`a`, `t3`, `user-17`), so there is no
  run to copy. Tried again: a wasm-only path for strings under 16 bytes (one scan, one
  `push_str`): fastest of 40 x 2000, six alternating pairs, base 44.9 to 50.7 us, new
  46.7 to 51.3 us: no gain, dropped. What is left in the 42 us is `format!` for `user-N`
  and `tN` (about 30% of the profile above), allocation and drops (20%).
- **workerd cold start: 39 vs 24 to 27 ms.** Taken apart below: about 7 ms is the wasm module
  being there (not its size), 7 ms the first request; a 25 KB smaller wasm moved nothing.
- **Deno through `Deno.serve`: -21%** on `/` and `/params` (72,973 vs 92,507; 50,809 vs
  64,674), ahead on `/list1000` (1.9x) and level on `/json-big`. The shim costs 2.4 us a
  request over a bare `Request` + `Response` (above), and Hono's request is 10.8 us at that
  rate, so 2.4 us is 18 to 22%. The default on Deno is the raw path, which wins.
- **Node through `node:http`: -15%** on `/` (46,129 vs 54,327), the opt-out path; the
  default raw path is 1.9x Hono there.
- **json-big on Node raw is +6% only** (20,812 vs 19,677): the same serializer, with no
  workerd tax, against V8's `JSON.stringify`.

### Bun, and the wasm that grew (main c3d610b, 2026-10-04)

Bun 1.4.2, same method as the Deno rows (`ab.mjs --group bun`, alternating, median of 5 x 10 s,
c=64, cold start median of 9; `hono/bun.ts` is `Bun.serve({ fetch: app.fetch })`). Wisp / Hono
rows are in the table above. Raw sockets win every route (+4% on `/`, 3.0x `/list1000`, +2%
`/json-big`, +48% `/params`); nothing to fix. `Bun.serve` loses on `/` (-35%: its shim costs
more per request than Bun's own `Response` path, as on Deno) and wins the rest; raw is the default.

Wasm size, the bench app, stripped, opt-level 3, built at successive commits (the file went
540,040 at bc5ce22 to 582,857 at c3d610b; the 627 KB quoted above did not reproduce: this
app builds to 582,857 at c3d610b). By symbol sizes of the unstripped builds: removing the admin/blob/idem code
(a56d2f9) took it to 488,775; jobs on the edge (09b87c5) put 100 KB back at that moment
(`wisp::edge` +14 KB, BTreeMap code +27 KB, admin +12 KB), later trims took some away; edge
WebSockets (13f6851) +29 KB; i18n (`seo`, `export`) +5 KB. The code that is only for
Node/Bun/Deno, the server loop over raw connections (`edge::connection`, `Raw`, the
`wisp_conn_*` exports, the raw WebSocket frames), was linked into every build: 43 KB.
`wisp build` now sets `WISP_REQUEST_ONLY=1` for Cloudflare, Pages, Vercel and Netlify (read by
`crates/wisp/build.rs`, a `cfg`; an older `wisp` ignores it), which leaves it out:

| | before | after |
|---|---|---|
| bench app, Cloudflare build, opt 3 | 584,353 | 541,063 |
| tests app (wasm32 release, stripped) | 1,814,667 | 1,772,227 |

Native `.text` of the tests app is unchanged (the edit is `cfg(wasm32)`; two builds of
main differ from each other in the same few functions, mine equals one of them).
Node, Bun and Deno builds are as before. `tests/gate` (`cargo run -p wisp-gate`) (a CI step) fails over 1,810,000
bytes (the tests app after the cut, +2%).

workerd cold start, taken apart (process start to first response, median of 9, three rounds,
alternating; the machine is bimodal, 25 or 32 ms for the same bytes, so only the sums mean
something): an empty worker 23, the same plus the (unused) `app.wasm` as a module 31 to 32,
the bridge module alone 24, Wisp end to end 39 (before and after the cut), Hono 27. So: module
load 7 to 9 ms, first request (instantiate, `main`/init, the request path compiled on first
call) 6 to 7 ms. In Node (lazy compile, same V8): compile 0.8 ms, instantiate 0.15,
env + `main` 1.7, first request 3.8 beyond that, second 0.5. Module load does not follow
size: the opt-level `s` wasm (433 KB) loaded in 25 to 31 ms against 31 to 32 for opt 3 (559 KB),
within the noise. Gate: size only (`tests/gate` (`cargo run -p wisp-gate`)); a time bound would flake on this.

## workerd profiled (V8 CPU profile, 2026-10-04)

The profile is taken over workerd's inspector (`workerd serve config -i
127.0.0.1:9339`, then `Profiler.start` over CDP while oha runs; 50 us
sampling, self time divided by the requests served; the wasm built with
`CARGO_PROFILE_RELEASE_STRIP=none` so its functions have names). The
machine was quiet this time, so the numbers are about twice the clean
run's above.

What it found: the biggest JS cost of a Wisp reply was workerd's `Response`
constructor, which copies a `Uint8Array` body out of the wasm memory slowly
(`/json-big`: 10.1 us in `Response` against 5.9 for Hono's string body; a
`body.slice()` first cost 9.8 us in itself, so it is the copy, not the
constructor). Kept (`bridge.js`, `webSink`):

- A body whose `content-type` is text, JSON, JavaScript or XML goes to
  `Response` as a string: a fatal `TextDecoder` with `ignoreBOM` (native, and
  exact: what is not UTF-8 throws and is passed as bytes), then workerd encodes
  it. `Response` 10.1 to 6.1 us, the decode 1 us.
- The head's init holds a `Headers` (made once per head) instead of a plain
  object: about 1 us on `/` (workerd copies one into the Response without
  reading names off an object). Duplicate names (`set-cookie`) need no list
  case any more.

Same build of `app.wasm` (byte-identical: no Rust changed, so native is
untouched), alternating, c=64, median of 5 x 10 s, CPU us/request:

| | `/` | `/list1000` | `/json-big` | `/params` |
|---|---|---|---|---|
| before | 19,637 (3.6) 52.0 | 5,608 (12.2) 182 | 10,753 (6.7) 94.9 | 18,632 (3.7) 54.5 |
| after | 20,215 (3.5) 50.5 | 6,126 (13.7) 165 | 11,330 (10.7) 89.5 | 18,947 (3.7) 55.0 |

(The second app of a pair runs about 1 us and 1.5% better with the same
build in both, so `/` and `/params` moved by no more than that.)

Against Hono, same run, median of 5 x 10 s, req/s (p99 ms) CPU us; cold start median of 9:

| | `/` | `/list1000` | `/json-big` | `/params` | cold start |
|---|---|---|---|---|---|
| Wisp | 19,645 (4.2) 52.5 | 6,033 (15.6) 170 | 11,441 (10.4) 88.3 | 19,258 (3.6) 53.5 | 25 ms |
| Hono | 20,908 (4.7) 49.1 | 2,359 (32.7) 429 | 11,894 (7.1) 86.2 | 20,256 (21.9) 51.6 | 19 ms |

`/json-big` went from -21% (the same quiet machine, before) to -4%; `/` is -6%.

What is left, from the profiles (self time, us per request):

- `/` (3.4 us of CPU): `Response` 6.6 against Hono's 4.1. Hono's `c.text`
  makes `new Response('hello')` with no init (workerd adds its own
  `text/plain;charset=UTF-8`); Wisp sends its own head, and workerd's cost of
  taking any init's headers is the 2.5 us. Then the bridge (`serve` 1.7,
  `enter` 1.3, `webSink` 0.9, `reply` 0.8) and the wasm (about 2 us) against
  Hono's router (about 4 us of JS).
- `/json-big` (2 us of CPU, inside the noise): the rest is wasm compute. Of
  the 50 us in wasm, the app's own `format!` (`user-{i}`, `t{}`:
  `fmt::write` 4.7, `String::write_str` 5.5, `Display` 2.9, `pad_integral` 2.0)
  is 15 us, dlmalloc (`free` 4.4, `malloc` 2.4, `unlink_chunk` 2.1) and
  dropping the rows (3.6) 13 us, the JSON string escape (`live::string`) 7.2 us
  for 600 strings (12 ns each), the rest of the serializer inlined into the
  route (`http::decide`, 14.8 us). Hono's `JSON.stringify` and template strings
  run in V8's C++ and JIT.
- Cold start: 25 against 19 ms; unchanged by this (the module load and the
  first request's lazy compile, as measured above).

### `/` without an init, and what did not pay (2026-10-04)

A 200 whose only header is `content-type: text/plain; charset=utf-8` goes to
workerd as `new Response(string)` with no init, as Hono's `c.text` does:
workerd adds `text/plain;charset=UTF-8` itself (the same type), and the init
was the 2.5 us the profile above found. `app.wasm` byte-identical (no Rust
changed, native untouched). Same busy machine, alternating, c=64, median of
5 x 10 s, req/s (CPU us):

| `/` | before | after |
|---|---|---|
| after second | 14,960 (67.3) | 15,477 (68.5) |
| after first | 14,274 (70.6) | 14,925 (67.7) |

+3.5% and +4.6%, both orders (the second of a pair runs about 1.5% better).

Measured and not kept (`/json-big`, cold start median of 21, a noisy machine):

- `opt-level = "s"` for the Cloudflare wasm: 527 to 410 KB, cold start 40 to
  39 ms, `/json-big` -2%. Inside the noise; the speed is the rule.
- `wasm-opt -O3` (binaryen, via npx, not a dependency): 527 to 459 KB, cold
  start 42 to 41 ms; `/json-big` too noisy to call (best run 108.5 to 101.5
  CPU us). About 1 ms of cold start, not the 6 ms gap.

Left: `/json-big` is wasm compute (the bench app's `format!` and dlmalloc,
above; a faster allocator needs a dependency with `unsafe`), cold start is the
module load and the first request's lazy compile.

### Linux, quiet box: Hono, wasm-opt, and where cold start goes (2026-10-04)

The Windows machine had other builds running, so this ran on a 4-core Linux
VPS (workerd from npm `workerd@latest`, node 24, oha 1.16). It is slower than
the desktop but nothing else ran on it. Same `workerd.mjs`, c=64, median of
5 x 10 s, cold start median of 21:

| | `/` req/s (CPU us) | `/json-big` req/s (CPU us) | cold start |
|---|---|---|---|
| Wisp (with the plain-text `Response`) | 6,670 (152.7) | 3,464 (294.6) | 60 ms |
| Hono | 7,377 (139.9) | 3,766 (269.9) | 43 ms |

**Cold start, split.** The time from spawn until the inspector answers is
`up`: the config is loaded and every module compiled. Then the first request
runs under the CPU profiler. Median of 15, ms:

| | up | first request | second request |
|---|---|---|---|
| a worker with no wasm (`new Response('x')`) | 33.7 | 3.1 | 1.3 |
| the same, with `app.wasm` listed as a module | 40.2 | 2.4 | 1.1 |
| Wisp | 38-41 | 17-18 | 2.5 |
| Hono | 34-38 | 6.7-8 | 1.3-2.2 |

So about 6 ms is workerd compiling (validating) the 527 KB module at load,
and about 11 ms is the first request. That 11 ms is not Wisp's init. In node
(same V8), a second instance of the same compiled module answers its first
request (instantiate, `main`, `prepare`, the request) in 0.8-2 ms, against
11-13 ms for the first instance. It is V8 lazily compiling the wasm functions
the first request touches. The profile's top frames are the big inlined ones
(`edge::poll` 3.5 ms, the `request` closure 2.0, `http::decide` 0.9,
`edge::start`'s closure 0.7). Hono's 7 ms is the same thing for its JS
(router building and lazy JS compile).

**wasm-opt (binaryen version_133) does not pay**, so `wisp build` does not run
it. Every level shrinks the file but slows the first request: it inlines into
bigger functions, which V8 then compiles whole. `/json-big` does not move
(in-process, alternating rounds, median us/request: rustc 127.5, -O1 130.8,
-O2 124.5, -O3 132.1, -Os 126.4, -Oz 125.5; noise about 3%). Cold start, ms:

| | bytes | up | first request | cold total |
|---|---|---|---|---|
| rustc (opt-level 3) | 526,774 | 38.0-41.3 | 16.9-18.3 | 55-59 |
| -O1 | 482,861 | 38.7 | 19.3 | 58.0 |
| -O2 | 476,581 | 40.1 | 22.0 | 62.1 |
| -O3 | 458,701 | 37.1-43.0 | 20.4-24.1 | 57.5-67.1 |
| -O4 | 460,640 | 37.2-42.1 | 20.5-22.6 | 57.7-64.6 |
| -Os | 458,243 | 38.9 | 20.6 | 59.5 |
| -Oz | 457,882 | 42.2 | 21.8 | 64.0 |

`--converge` with -O3 and -O4 gives 457,769 and 459,747 bytes, with the same
times.

**opt-level `s`** for the Cloudflare wasm: cold start 51-56 against 58-59 ms,
but `/json-big` 140 against 130 us in wasm (-8%). Level `2` brings no cold
start gain and is 14% slower. The speed is the rule, so `3` stays.

**`/json-big`**: the serializer already writes into one reused buffer
(`Response::json_of` takes `http::spare()`) with no intermediate `String`s,
so there is nothing to cut there. `["user-", &i.to_string()].concat()` in
place of `format!("user-{i}")` measured the same (Windows, both orders), so
the docs keep `format!`. What is left, per request:

- the app's own allocations, 600 `String`s and 200 `Vec`s made and dropped:
  dlmalloc `free` 6.1 us, `malloc` 2.0, `unlink_chunk` 1.8, drop 2.4;
- `fmt` 13.5 us;
- the escape 7.4 us (12 ns a string).

A per-request arena would remove most of the allocator's share, but a
`GlobalAlloc` needs `unsafe`.

### Warm-up at load, and what did not pay (2026-10-04)

**Kept: a warm-up request while the worker loads** (`worker.js`). V8
compiles wasm lazily, a function at its first call, and every instance of a
module shares that code. So the worker makes a throwaway instance at module
scope and sends it one request (`/_wisp/warm`, a 404 that runs the request
path); the real instance, made at the first `fetch` with its `env`, then runs
compiled code. Linux VPS (busy: another build ran), workerd, `/json`, median
of 15, ms, two runs each (`up` is spawn to the port open; then 50 ms of
quiet; `first` is the first request):

| | up | first request | spawn to first response, no pause |
|---|---|---|---|
| Wisp before | 27.4-29.3 | 16.2-17.5 | 50.4-56.4 |
| Wisp, warm-up | 28.9-30.8 | 4.8-5.8 | 55.4-57.5 |
| Hono | 23.7-25.7 | 6.8-8.3 | 38.2-40.9 |

**Since narrowed: the warm-up runs no app code.** That request went through
the app's `init`, `before` hook, logs and error page, on a phantom request at
global scope, where Workers forbid random numbers and I/O. The warm-up
instance is now given `WISP_WARM_UP=1`: the runtime skips `init` and the
sessions, and answers the request itself (parse, route, the page shell,
serialize, the reply to the host). Same VPS, busy, `/json`, median of 15, ms,
first request: none 16.5 / 23.9-25.1, the old warm-up 5.5 / 7.9-8.8, the
narrowed one 11.5 / 16.1-18.1. It keeps about half the gain; the rest is the
app's own code (`handle`, which runs the `before` hook first, and the
templates), which a warm-up must not run.

On Cloudflare a worker is started during the TLS handshake, so what a user
waits for is the first request: Wisp now answers it faster than Hono. A
request that arrives before the load is done waits for the warm-up as before
(the last column: the same within noise). Steady state does not change.

**Tried, slower: another allocator.** In node, fastest of 40 x 2,000 calls,
us per request, dlmalloc (std's) / rlsf `SmallGlobalTlsf` / talc 5.1
`WasmDynamicTalc`: `/json-big` 46.3-47.9 / 53.6-61.1 / 47.8-49.3, `/` 4.77 /
5.06 / 5.47, `/params/42` 7.25 / - / 12.98. Both shrink the wasm by 54-59 KB;
neither is faster, so dlmalloc stays.

**Tried, no gain: per-crate opt-levels.** `opt-level = "s"` for every
dependency and `3` for `wisp` and the app: 523,345 against 525,535 bytes
(-0.4%), `/json-big` 45.5-47.2 against 45.5-48.1 us (noise). With fat LTO
and one codegen unit the merged module is optimized again at the top level,
so per-crate levels barely reach the output; `wisp build` keeps `3`.

## workerd on Linux, pinned, 2026-10-04 (VPS, 4 vCPU, idle)

workerd 2026-10-04 on core 0, `oha -c 16` on cores 2-3, 7 s runs, Wisp and Hono
alternating, median of 7 (`/`) and 5 (`/json-big`). Per request: user
instructions (`perf stat`, the stable measure here: CPU time and cycles swing
20% between runs of the same build on this host), workerd's CPU and req/s.

| | `/` instr | `/` CPU, req/s | `/json-big` instr | `/json-big` CPU, req/s |
|---|---|---|---|---|
| Hono | 265 k | 217 us, 4,592 | 714 k | 346 us, 2,884 |
| Wisp | 221 k | 200 us, 5,003 | 999 k | 359 us, 2,789 |
| `new Response('hello')`, no framework | 205 k | | | |

On a quiet machine Wisp's `/` is ahead of Hono's; the 10% behind of the
Windows tables above was that machine's noise. A profile (`perf record -g`, V8's
`--perf-basic-prof` through the config's `v8Flags`) of `/`, Wisp over Hono, in
us a request: the app's wasm +8 (turbofan code; nothing left in Liftoff), the
bridge's JS +4 (`serve`, `reply`, `enter`, the `subarray` and string concat),
tcmalloc +2; Hono's own JS (object literals, property definition: V8's
`PropertyDescriptor`, `MigrateToMap`, dictionary adds) costs it more than that.
The rest (kj's event loop, `IoContext`, `Headers`) moves by 5 us between runs of
one build, both ways.

`/json-big` (wasm function names from the same build unstripped), share of
workerd's CPU: the route's closure in `http::decide` 9%, `fmt` (`write`,
`write_fmt`, `Display`, `pad_integral`, the app's `format!`) 8%, `live::string`
5%, dlmalloc 3.5%, dropping the rows 2.4%. The Hono app builds the same rows
with the same strings, so the two do the same work.

Measured and kept as they are:

- Compatibility date 2025-01-01 (Hono's `wrangler.toml`) for Wisp, against
  2025-09-01: 225 k against 221 k instructions, no gain. Neither sets flags.
- The warm-up at load. Start to first response, then a second one, median of 15:
  as it is 74.4 ms, 4.3; without it 70.8 ms, 4.8; deferred into `ctx.waitUntil`
  after the first request 76.8 ms, 4.7 (Hono 53.4). With 30 ms idle before the
  first request (a TLS handshake), the first request takes 8.6 ms with the
  warm-up, 23.8 without and 26.1 deferred (Hono 12.2). That idle time is what
  Cloudflare has, so the warm-up stays at load.

## Ranking against the popular frameworks, 2026-10-04 (`bench/rank/`)

Where does Wisp place on each host, against what people actually deploy? Same
four routes as above (`/` text, `/list1000` HTML with 1,000 escaped items,
`/json-big` 200 objects, `/params/42?q=hello%20world&x=1` with `cookie: sid=abc123;
theme=dark`), each framework written the way its docs write it (its own router,
cookie helper, template or JSX; production build; `NODE_ENV=production`), and
every app is checked for the same output before it is measured (exact text, the
same JSON, a thousand escaped `<li>`; the HTML shells differ by framework, which
is why Fresh, Next, SvelteKit and Astro carry their own head and hydration
markup). `apps/` holds the apps, `rank.mjs` the harness, `report.mjs` the tables.

Machine: the 4 vCPU Linux VPS (AMD EPYC 7B13, Ubuntu 24.04), idle (load under
0.1 before the run), nothing else on it. Servers pinned to cores 0-1, `oha` to
cores 2-3. `oha` 1.16.0, c=64, 3 s warm-up, 5 x 10 s, **median**; cold start is
spawn to the first complete `GET /` (median of 15); memory is RSS of the server
and its children after all four routes were loaded. All of a host's servers run
at once and each route's runs alternate between them, so noise hits all alike.
Node 24.9.0, Bun 1.4.2, Deno 2.9.7, workerd 1.20261001.1 (bare `workerd serve`,
not `wrangler dev`, whose proxy caps everything near 1.7k req/s; `wrangler
deploy --dry-run` bundles each worker, a stub `ASSETS` binding answers 404).
Hono 4.13.13 (+ @hono/node-server 2.1.3), itty-router 5.0.24, Fastify 5.12.5
(+ @fastify/cookie), Express 5.2.1 (+ cookie-parser), Elysia 1.4.30, Oak 17.2.0,
Fresh 2.3.3, SvelteKit 3.0.0 (adapter-node 6.0.0, adapter-cloudflare 8.0.0),
Astro 7.3.5 (@astrojs/cloudflare 14.3.3), React Router 8.4.0 (@cloudflare/vite-plugin),
Next 16.3.8 (standalone on Node; @opennextjs/cloudflare 1.20.8 on workerd).
Wisp is this tree's `wisp build --target node|bun|deno|cloudflare`, with the same
`apps/wisp` routes as above. "wisp raw" is the default (the runtime's own
sockets); "wisp node:http", "Bun.serve" and "Deno.serve" are `WISP_NODE_HTTP=1`.

Rebuild everything: `WISP=<wisp binary> bench/rank/build-wisp.sh` (anywhere with
the Rust toolchain), then on the Linux box `bench/rank/build.sh` and
`bench/rank/run.sh`; the raw numbers are `bench/rank/results/*.json`.

Reading the tables: each cell is req/s with Wisp's place in parentheses. A place
counts the other frameworks only; the two Wisp variants do not push each other
down. Ordered by mean place over the four routes. Cold start and memory: lower
is better. Throughput swings 15% to 30% between runs on a VPS (the five runs of
each cell are in the JSON), so a gap under about 10% is a tie, not a loss.
Nothing in the requested list was skipped (Next on workerd built through
OpenNext). Fresh was built with `vite build` and served with `deno serve`.
Next's route handlers are slow on every host here (about 1k
req/s on Node for `"hello"`), and 19 req/s for 1,000 JSX items on workerd.

### workerd

c=64, 10 s runs, median of 5, cold start median of 15; 2026-10-04. Each cell: req/s (place among the frameworks; a second Wisp variant is not counted against the first).

| framework | `/` | `/list1000` | `/json-big` | `/params` | cold start ms | RSS MB after load |
|---|---|---|---|---|---|---|
| hono | 6,186 (#1) | 795 (#3) | 3,359 (#1) | 4,774 (#1) | 75 (#2) | 303 (#1) |
| **wisp** | **5,579 (#2)** | **1,902 (#1)** | **3,243 (#2)** | **4,771 (#2)** | **100 (#3)** | **374 (#2)** |
| itty | 4,766 (#3) | 855 (#2) | 3,002 (#3) | 4,130 (#3) | 70 (#1) | 754 (#6) |
| astro | 2,986 (#4) | 698 (#4) | 2,203 (#5) | 2,477 (#4) | 179 (#6) | 477 (#4) |
| sveltekit | 2,884 (#5) | 418 (#5) | 2,296 (#4) | 2,223 (#5) | 120 (#4) | 1581 (#7) |
| react-router | 2,351 (#6) | 226 (#6) | 1,905 (#6) | 1,849 (#6) | 151 (#5) | 425 (#3) |
| next | 457 (#7) | 45 (#7) | 384 (#7) | 399 (#7) | 631 (#7) | 631 (#5) |

### Node

c=64, 10 s runs, median of 5, cold start median of 15; 2026-10-04. Each cell: req/s (place among the frameworks; a second Wisp variant is not counted against the first).

| framework | `/` | `/list1000` | `/json-big` | `/params` | cold start ms | RSS MB after load |
|---|---|---|---|---|---|---|
| **wisp raw** | **33,942 (#1)** | **3,165 (#1)** | **7,959 (#1)** | **30,533 (#1)** | **166 (#1)** | **82 (#1)** |
| **wisp node:http** | **13,151 (#3)** | **3,001 (#1)** | **5,940 (#1)** | **11,627 (#2)** | **152 (#1)** | **290 (#5)** |
| hono | 14,196 (#2) | 1,043 (#3) | 4,583 (#2) | 9,131 (#3) | 167 (#2) | 100 (#3) |
| fastify | 13,661 (#3) | 1,049 (#2) | 4,496 (#3) | 11,862 (#2) | 312 (#5) | 95 (#2) |
| express | 7,325 (#4) | 927 (#4) | 3,484 (#4) | 6,671 (#4) | 231 (#4) | 226 (#4) |
| sveltekit | 4,802 (#5) | 516 (#5) | 2,629 (#5) | 4,300 (#5) | 187 (#3) | 269 (#5) |
| next | 1,211 (#6) | 66 (#6) | 1,040 (#6) | 1,227 (#6) | 725 (#6) | 505 (#6) |

### Bun

c=64, 10 s runs, median of 5, cold start median of 15; 2026-10-04. Each cell: req/s (place among the frameworks; a second Wisp variant is not counted against the first).

| framework | `/` | `/list1000` | `/json-big` | `/params` | cold start ms | RSS MB after load |
|---|---|---|---|---|---|---|
| **wisp raw** | **49,417 (#1)** | **4,082 (#1)** | **8,630 (#1)** | **47,525 (#1)** | **74 (#2)** | **40 (#2)** |
| **wisp Bun.serve** | **24,934 (#3)** | **3,369 (#1)** | **6,525 (#2)** | **21,358 (#3)** | **75 (#2)** | **42 (#2)** |
| elysia | 48,256 (#2) | 924 (#2) | 6,559 (#2) | 23,468 (#3) | 177 (#3) | 50 (#3) |
| hono | 45,735 (#3) | 874 (#3) | 5,846 (#3) | 30,840 (#2) | 58 (#1) | 39 (#1) |

### Deno

c=64, 10 s runs, median of 5, cold start median of 15; 2026-10-04. Each cell: req/s (place among the frameworks; a second Wisp variant is not counted against the first).

| framework | `/` | `/list1000` | `/json-big` | `/params` | cold start ms | RSS MB after load |
|---|---|---|---|---|---|---|
| **wisp raw** | **39,542 (#2)** | **2,403 (#1)** | **3,939 (#2)** | **34,815 (#1)** | **87 (#2)** | **86 (#2)** |
| hono | 43,060 (#1) | 860 (#2) | 3,825 (#3) | 28,454 (#2) | 56 (#1) | 51 (#1) |
| **wisp Deno.serve** | **22,847 (#2)** | **2,260 (#1)** | **3,485 (#4)** | **16,011 (#2)** | **75 (#2)** | **69 (#2)** |
| fresh | 19,357 (#3) | 519 (#4) | 4,352 (#1) | 7,154 (#4) | 90 (#3) | 98 (#4) |
| oak | 15,011 (#4) | 750 (#3) | 3,518 (#4) | 10,557 (#3) | 154 (#4) | 86 (#3) |

### Where Wisp is below 3rd or behind Hono

Gap is how far behind in %: lower req/s, or higher cold start and memory. Gap to 3rd is against the framework in 3rd place (Wisp not counted).

| host | Wisp variant | metric | place | Wisp | Hono | behind Hono | behind 3rd |
|---|---|---|---|---|---|---|---|
| workerd | wisp | `/` | #2 | 5,579 | 6,186 | 10% | - |
| workerd | wisp | `/json-big` | #2 | 3,243 | 3,359 | 3% | - |
| workerd | wisp | `/params` | #2 | 4,771 | 4,774 | 0% | - |
| workerd | wisp | cold start ms | #3 | 100 | 75 | 34% | - |
| workerd | wisp | RSS MB after load | #2 | 374 | 303 | 23% | - |
| Node | wisp node:http | `/` | #3 | 13,151 | 14,196 | 7% | - |
| Node | wisp node:http | RSS MB after load | #5 | 290 | 100 | 190% | 28% (express) |
| Bun | wisp raw | cold start ms | #2 | 74 | 58 | 28% | - |
| Bun | wisp raw | RSS MB after load | #2 | 40 | 39 | 3% | - |
| Bun | wisp Bun.serve | `/` | #3 | 24,934 | 45,735 | 45% | - |
| Bun | wisp Bun.serve | `/params` | #3 | 21,358 | 30,840 | 31% | - |
| Bun | wisp Bun.serve | cold start ms | #2 | 75 | 58 | 29% | - |
| Bun | wisp Bun.serve | RSS MB after load | #2 | 42 | 39 | 8% | - |
| Deno | wisp raw | `/` | #2 | 39,542 | 43,060 | 8% | - |
| Deno | wisp raw | cold start ms | #2 | 87 | 56 | 55% | - |
| Deno | wisp raw | RSS MB after load | #2 | 86 | 51 | 69% | - |
| Deno | wisp Deno.serve | `/` | #2 | 22,847 | 43,060 | 47% | - |
| Deno | wisp Deno.serve | `/json-big` | #4 | 3,485 | 3,825 | 9% | 1% (oak) |
| Deno | wisp Deno.serve | `/params` | #2 | 16,011 | 28,454 | 44% | - |
| Deno | wisp Deno.serve | cold start ms | #2 | 75 | 56 | 34% | - |
| Deno | wisp Deno.serve | RSS MB after load | #2 | 69 | 51 | 35% | - |

### Cold start and memory, second pass (2026-10-04)

workerd and Node above are a rerun after two changes; Bun and Deno are the
first run. `rank.mjs` now alternates cold starts between the servers, a round
at a time, as it does the routes: run back to back, a busy minute fell on one
framework (workerd's Wisp read 98 ms, #5, then 100 ms, #3, with sveltekit at
120). Node's raw server checked its sockets with `fetch`, which loads undici at
first use; it asks with node:http now (loaded already): 153 to 131 ms, Hono 128
(interleaved, median of 21).

workerd cold start, taken apart (process start to first response, interleaved,
median of 31): an empty worker 49 ms, the same worker importing `app.wasm` 57
(+7, V8 decoding and validating 550 KB), instantiating it 59 (+3), the first
request 77 (+18), the warm-up instance 82 (+4). Of the first request, about
12 ms is Liftoff compiling the 97 functions (148 KB) it calls: Node with
`--no-wasm-lazy-compilation` answers it in 6 ms against 19 lazily, and
`--trace-wasm-compilation-times` lists them (`edge::request` 31 KB, `http::decide`
17 KB, `start` 12 KB, `http::parse` 11 KB, `settle_plain` 9 KB). Hono's whole
start is 54 ms there, under the 57 that loading the module alone costs, so
beating it needs a much smaller wasm; `opt-level = "s"` (430 KB) took 4 to 7 ms
off and stays out for its steady-state cost (above). Boxing the WebSocket and
cron arms of `edge::request` left it at 31 KB: `http::answer`, inlined, is
the bulk.

Node's "RSS after load" swings run to run with the same build (node:http: 89,
181, 245 and 290 MB; raw: 82 to 120). The growth is outside JS and wasm: after
10 s of `/list1000` the JS heap is 26 MB and the wasm memory 1 MB, and
`MALLOC_ARENA_MAX=1` takes some off, so it is V8's compiler and code memory
held by malloc, not anything the shim keeps.

**What it says.** Wisp (raw sockets) is 1st on every Node and Bun route and on
Deno `/list1000` and `/params`; on Deno `/` it is 2nd (8% behind Hono, inside
the noise) and on `/json-big` 2nd (ahead of Hono, behind Fresh). It is 1st on
`/list1000` on every host (1.9x to 4.7x Hono; the escape and the list are one
pass over bytes). On
workerd it is 2nd to 3rd on throughput: `/list1000` is 1st (about 1.9x Hono),
`/` is 2nd, `/json-big` and `/params` are 3rd behind itty-router and Hono, by 9%
and 12% from Hono (the known cost of the wasm entry and `JSON.stringify`
running in V8's C++ for Hono, see above; both inside run-to-run noise on this
VPS). The `Bun.serve`, `Deno.serve` and `node:http` variants pay the host's
`Request`/`Response` objects and are the slow Wisp paths (45% behind Hono on
Bun `/`, 47% on Deno `/`): the raw path is the default, so those rows are the
"portable" option, not the headline.

**Shims after the lighter bridge (2026-10-05, same method, Wisp shim and Hono only).**
`Bun.serve` `/` 28,870 vs Hono 46,402 (38% behind, was 45%), `/params` 21,071 vs
29,678 (29%); `Deno.serve` `/` 29,784 vs 46,393 (36%, was 47%), `/params` 20,589
vs 31,131 (34%, was 44%). Server instructions a request on Deno (`perf stat`):
`/` 55k to 50k (Hono 32.5k), `/params` 77.5k to 73.7k (Hono 54k). What is left
(perf, `--perf-basic-prof`): the app's wasm, about 9k instructions for `/`, and
V8's wasm/JS crossings; on Bun `server.requestIP` alone is 16% of the profile, and
the peer is read eagerly by `Cx`.

**Where Wisp does not place 3rd or ahead of Hono.** Throughput: workerd `/`
(5% behind Hono), `/json-big` (9%), `/params` (12%); Deno `/` (8% raw, 47%
`Deno.serve`), Deno `Deno.serve` `/json-big` (4th, 9% behind Hono) and `/params`
(44%); Bun `Bun.serve` `/` (45%) and `/params` (31%); Node `node:http` `/` (7%).
Cold start is Wisp's weakest column: 98 ms on workerd (5th, 69% behind Hono's
58 ms; the wasm has to be compiled and warmed at load, see the warm-up
measurements above), 203 ms on Node raw (2nd, 8% behind Hono), 74 ms on Bun
(28%), 87 ms on Deno (55%). Memory after load: Node raw 120 MB against Hono's
95 (26%), Deno 86 against 51 (69%), Bun 40 against 39. Wisp has the smallest
footprint on workerd (309 MB, 1st) and with `node:http` on Node (89 MB, 1st).
The full list, with the gap to 3rd where Wisp is below it, is the last table.
