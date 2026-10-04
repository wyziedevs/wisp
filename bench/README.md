# Benchmarks

Wisp against the ten most popular web frameworks and the ten fastest
(TechEmpower's top tier), on the same machine, doing the same work; and,
with `--suite benchmarker`, against the-benchmarker's top ten on its routes
and load (below).

- Popular: ASP.NET Core, Actix Web, Axum, Go's net/http, Gin, Fiber,
  Express, Fastify, SvelteKit, Next.js.
- Fastest: Actix Web, may-minihttp, xitca-web, ntex, hyper, fasthttp,
  Vert.x, uWebSockets.js, Bun, Elysia.
- the-benchmarker's top ten (`--group top`, suite only): Caprese,
  uWebSockets.js, fulmine.js, may-minihttp, jet_server, Ohkami, MoroJS
  engine, ActiveJ.

```
cargo run -r -p bench-run -- [-c 64] [-d 10] [-w 5] [--rounds 1]
    [--pipeline 1] [--group fast,popular,top|all] [--only wisp,actix]
    [--paths fortunes] [--no-build] [--csv results.csv]
    [--suite benchmarker|real] [--think 1.0] [--max-users 50000]
    [--slo-ms 100] [--tests users,churn,...] [--soak 60]
```

Runs on Linux and Windows. Needs Rust; a server whose toolchain (the .NET 10
SDK, Go, Node, Bun, a JDK and Maven, Nim, Dart) is missing, or whose build
fails, is skipped with a note. may-minihttp, uWebSockets.js, Bun, Elysia,
fulmine.js, MoroJS engine, Caprese and jet_server run on Linux only
(may-minihttp's Windows I/O answers a kept-alive connection's first request
again; Caprese and jet_server are written for Linux; the others share the
port with SO_REUSEPORT). `--group` picks a list or several (all by default),
Wisp always included;
`--only` and `--paths` narrow it by substring, `--rounds` runs every server
that many times, taking turns, and reports the mean, and `--extra
NAME=COMMAND` adds a server of your own (it gets `PORT` and `THREADS`, and
is measured on `/plaintext`). The table is sorted fastest first on each
path. Below it, `rank` lines give Wisp's place: on each path in requests per
second and in CPU per request (each line names how the load was sent), then
on peak memory, time to first response and deploy size. `--pipeline N` sends
N requests back to back on every connection before reading their N
responses, as TechEmpower's plaintext does with 16; it is off by default,
since browsers do not pipeline, and applies to every path that runs (use
`--paths plaintext`). Pipelined and closed-loop numbers are not comparable.

Every server answers the same four paths. `/fortunes` is TechEmpower's
fortunes test without the database: copy 12 rows, add one, sort by message,
render an HTML table with escaping. `/plaintext` returns `Hello, World!`,
and `/json` serializes `{"message":"Hello, World!"}` per request. `/page` is
a realistic server-rendered page: a layout (head, nav, footer), an `<h1>`, a
table of 50 rows built per request, each with an id, a name containing `<&"`
to escape, a number and a class (`on` or `off`) chosen by a boolean, and a
small form. Each server writes it the way its own docs would: a layout and a
page in the framework's template language where it has one, a string builder
or template literal where its TechEmpower entry has none. The runner checks
that every server sends the same page before measuring it: the parts above,
and every row's class, id, number and unescaped name, ignoring whitespace,
comments and how each template spells an escaped character (`&quot;`,
`&#34;`, or none in text). None caches a response on these four (Wisp's
cached and baked pages are paths of their own: see `app/` below).

- `app/`: the Wisp side. `/page` is a `+layout.wisp` in a route group
  (`(site)`, so `/fortunes` is not wrapped) and a `+page.wisp` with its Rust
  block. It also answers `/messages/1`, `/json`'s object
  as a `#[derive(Rest)]` row through the resource's own GET (lock, JSON,
  ETag): on Windows (8 server cores, mean of 3 rounds) 8.2 µs of CPU a
  request to `/json`'s 7.7, the half microsecond being the table's lock,
  the ETag and its 33 bytes. `/fortunes-cached` is `/fortunes` with
  `const CACHE: u32 = 1;` (each worker renders it once a second), and
  `/static` is its table written out as a page with no holes, which the
  build bakes into the binary with its response head. Both are Wisp's
  alone, beside the uncached `/fortunes` everyone renders.
- `aspnet/`: written the way the ASP.NET Core docs and templates do:
  `/fortunes` as a Razor Page, `/page` as one with a `_Layout`,
  `/fortunes-blazor` as a Blazor component (static SSR), `/plaintext` and
  `/json` as minimal APIs. Logging is set to
  `Warning` as the templates' appsettings do, and the HTML encoder emits
  non-ASCII as is, like Wisp.
- `rust/`: Actix Web, Axum, may-minihttp, xitca-web, ntex and bare hyper
  (a tokio runtime per thread, SO_REUSEPORT) in one binary (`bench-rust
  actix|axum|may|xitca|ntex|hyper|ohkami`), all rendering with Askama, which
  compiles templates to Rust as Wisp does (`/page` inherits `templates/`'s
  layout), and serializing with serde; and Ohkami on its nio runtime. A
  Cargo workspace of its own, so none of it reaches Wisp's `Cargo.lock`.
- `go/`: net/http, Gin, Fiber v3 and bare fasthttp in one binary
  (`bench-go nethttp|gin|fiber|fasthttp`), all rendering with
  `html/template` (Gin's and Fiber's html renderers wrap it; `/page`'s
  layout is a `define`) and serializing with `encoding/json`.
- `java/`: Vert.x 4.5 on epoll, a verticle per event loop, the pages from a
  `StringBuilder` as its TechEmpower entry does; ActiveJ 5.5 in the same
  jar (`activej`). `mvn package`.
- `nim/`: Caprese, built as its Dockerfile there builds it (`NOSSL=1
  nimble install`, `nim c -d:release --opt:speed` with LTO).
- `dart/`: jet_server (FFI and epoll), compiled ahead of time.
- `node/`: Express (its defaults) and Fastify, the pages from template
  literals (`fortunes.mjs`, `page.mjs`: no template engine, the fastest
  path), SvelteKit with adapter-node (`+page.server.js` and `+page.svelte`,
  `/page` in a `(site)` route group with a `+layout.svelte`), Next.js with
  the App Router (server components forced dynamic, so they render per
  request like the rest, not once at build; `/page` under a `(site)` layout),
  and uWebSockets.js. Each runs under
  `cluster.mjs`, one process per server CPU, as `pm2 -i` does. Bun
  (`Bun.serve`) and Elysia use the same page and run under
  `bun-cluster.js`, one process per CPU sharing the port. fulmine.js
  (Express's API on uWebSockets.js, forking its own workers) and MoroJS
  engine (a native HTTP engine) answer only the-benchmarker's routes.
- `run/`: the runner, `bench-run`.
- `load/`: the load generator, a library the runner calls and a command of
  its own (`wisp-load http://127.0.0.1:3000/ -c 64 -d 10`). It is
  closed-loop: each connection sends a request and waits for the whole
  response before the next (no pipelining), like wrk and bombardier by
  default; `--pipeline 16` sends 16 at once instead, timing each response
  from the first byte of its batch. It records latency in a log-linear
  histogram and counts only requests that complete in the measured window.

The practice routes, which stress what real apps do, on Wisp, Express,
Fastify, Bun, SvelteKit, Next.js, ASP.NET Core, Go's net/http, Actix and Axum,
each written the way its framework's docs would:

- `GET /wait`: waits 20 ms without holding a thread (a database call's
  stand-in), then `200 {"ok":true}`.
- `POST /echo`: body `{"name","email","age","tags"}`, valid when name is 1 to
  50 characters, email has an `@`, age 0 to 150 and tags at most 10. Valid:
  `200` with the same four fields in that order. Invalid or unparsable:
  `422 {"errors":["name",...]}` in the order name, email, age, tags (`["body"]`
  for a body that does not parse).
- `POST /upload`: a body of any type up to 8 MiB, answered `200 text/plain`
  with its byte count; larger is a `413`.
- `GET /list`: 1000 objects `{"id":i,"name":"user i","email":"user{i}@example.com",
  "active":i%3!=0}` built and serialized per request.
- `GET /static/app.js`: `static/app.js` (about 100 KB) from the framework's
  own static-file facility with its default cache headers. Wisp, SvelteKit
  and Next.js serve only from their own `static`/`public` folder, so
  bench-run copies it there before building them (the copies are not
  tracked); Next.js's standalone build needs `public` copied beside `server.js`
  (its docs say so), and SvelteKit needs `BODY_SIZE_LIMIT=8M` at launch.
- `GET /ws`: a WebSocket that echoes each message. Not on SvelteKit and
  Next.js, which have none built in.

`bench-run` builds everything in release mode and runs each server alone,
pinned to half of the CPU cores, with the load generator on the other half
(whole cores each, so the two never share a core's hyperthreads). Every
server gets as many threads or processes as it has CPUs. Before measuring,
it checks that each `/fortunes` sends the same 13 rows in the same order,
escaped, with the non-ASCII row as is, and that `/page` is the page above.
Besides throughput and latency it reports the server's CPU time per request
(throughput alone can be capped by the load generator or the OS network
stack), peak memory, the time from launch to the first response, and its
deploy size. CPU time and memory include every process a server starts.
Deploy size is the binary or app directory you would copy, without the
runtime (.NET, the JVM, Node, Bun and Dart's are installed apart): Wisp's
binary, ASP.NET's publish directory, Caprese's and jet_server's binaries,
SvelteKit's `build/`, Next.js's standalone output, the `node_modules` of the
others. The Rust, Go and Java servers share one binary or jar among several
frameworks, so they have no size of their own. It prints the results as a
Markdown table, like the one below.

Only sources are tracked here; builds, `node_modules` and lockfiles are
ignored.

## the-benchmarker suite

The board most people look at is the-benchmarker's web-frameworks
([repo](https://github.com/the-benchmarker/web-frameworks),
[results](https://web-frameworks-benchmark.netlify.app/result)).
`--suite benchmarker` measures what it measures. What that is, read from
their repo at 7980442 (2026-09-29) and the results of 2026-09-28 (a1107b4):

- **Routes** (README's "Benchmark contract", `.spec/route_spec.rb`): `GET /`
  answers a 2xx with an empty body, `GET /user/:id` a 2xx with the id,
  `POST /user` a 2xx with an empty body. They are loaded as `GET /`,
  `GET /user/0` and `POST /user` (`.env`: `ROUTES`).
- **Load** (`.tasks/config.rake`, `run.sh`): [zrk](https://github.com/zoxy-io/zrk)
  2.4 or later, `zrk --plain --closed -t THREADS -c N -d 15 -m METHOD
  --timeout 8s --format json`: closed loop over keep-alive connections, no
  pipelining, each connection sending its next request when its last
  response is in. THREADS is the count of load CPUs (12). zrk's request has
  `User-Agent: zrk`, `Connection: keep-alive`, and `Content-Length: 0` on
  the POST. One warmup per framework, `zrk --closed -c 50 -d 5s` on `GET /`;
  then each route for 15 s (`DURATION`) at 64, 256 and 512 connections
  (`CONCURRENCIES`), with no warmup of its own. The rate is requests over
  the whole run, connecting included (zrk's `achieved_rate`). The site's
  home page still says wrk with 8 threads; the harness moved to zrk in
  2026-09 (their README's "Why the figures moved").
- **Where** (`config.yaml`, `.env`): each framework in a Docker container of
  its own, built by its language's Dockerfile in release mode, started with
  `docker run -td --cpuset-cpus=0-3`: the default bridge network, the
  default seccomp profile, no published port (zrk loads the container's
  bridge address, port 3000), zrk pinned to CPUs 4-15 with `taskset`. The
  machine: 16 CPUs, 7 GB, Linux 7.2 (Fedora 44). `run.sh` per framework:
  build, 60 s sleep, contract test, warmup, collect, stop.
- **Ranking** (`.tasks/db.rake`, the results page): `data.json` keeps each
  metric's `avg` per framework and level, so a framework's rate at a level
  is the mean of its three routes. The results page sorts by "Requests /
  Second (64)", with 256 and 512 beside it. They also keep p50 to p99.99
  and a saturation probe (the container's CPU over its 4 cores): most of the
  top runs near 50%, so zrk, not the server, set the pace there, which their
  own `rake db:check_saturation` counts as a run to distrust.

Their top 15 of 388 on 2026-09-28, req/s at 64 / 256 / 512 connections:

| #  | Framework       | Language   |      64 |     256 |     512 |
|---:|-----------------|------------|--------:|--------:|--------:|
|  1 | caprese         | Nim        | 269,338 | 251,156 | 239,278 |
|  2 | uwebsockets     | JavaScript | 255,933 | 238,546 | 231,757 |
|  3 | fulmine.js      | JavaScript | 255,600 | 238,599 | 231,636 |
|  4 | may_minihttp    | Rust       | 254,539 | 226,369 | 218,158 |
|  5 | jet_server-vm   | Dart       | 240,851 | 251,249 | 244,779 |
|  6 | jet_server      | Dart       | 239,539 | 251,346 | 244,814 |
|  7 | ohkami-nio      | Rust       | 232,682 | 225,703 | 217,292 |
|  8 | morojs-engine   | JavaScript | 230,776 | 209,728 | 204,495 |
|  9 | ohkami-tokio    | Rust       | 230,163 | 215,284 | 212,694 |
| 10 | activej         | Java       | 222,819 | 203,238 | 198,183 |
| 11 | morojs          | JavaScript | 218,594 | 199,058 | 194,892 |
| 12 | hyper           | Rust       | 214,474 | 199,897 | 196,622 |
| 13 | breeze          | Go         | 213,463 | 196,845 | 191,137 |
| 14 | sifrr           | JavaScript | 212,654 | 198,599 | 194,631 |
| 15 | khttp           | Rust       | 212,390 | 198,445 | 195,663 |

The others here, at 64: Actix 16th, Elysia 23rd, Vert.x 32nd, Axum 35th,
fasthttp 38th, Fiber 44th, ASP.NET Core (minimal API) 66th, Bun 68th, Gin
141st, net/http 143rd, Fastify 189th, Express 238th, Next.js 362nd.
xitca-web, ntex, SvelteKit and Wisp have no entry.

The suite sends the same requests on the same routes at the same levels,
for the same time after the same warmup, and ranks by the same figure (the
mean of the routes at 64; 256 and 512 get rank lines too), with zrk and
their flags where zrk is installed (the Linux script installs 2.5.0), else
wisp-load sending zrk's bytes (a thread per connection, so at 512 it costs
the load CPUs more than zrk does). Before loading, each server must pass
their contract. Every rival answers the routes with its entry's code where
it has one there: Actix, Axum, hyper, may-minihttp, net/http, Gin, Fiber
(and its config), fasthttp, Express, Fastify, uWebSockets.js (declarative
responses, and `_cfg('silent')` for every path), Bun, Elysia, Next.js,
ASP.NET Core's minimal API and Vert.x; xitca-web, ntex and SvelteKit have
none and are written as their docs would. Wisp's are two `+server.rs`
files: `fn get() {}` at the root, and `fn get(id: String) -> Response {
Response::text(id) }` with `fn post() {}` in `user/`.

From their top ten, the ones missing here were added from their entries,
answering only the three routes: Caprese (Nim), fulmine.js, jet_server
(Dart, the AOT build), Ohkami (on nio), MoroJS engine and ActiveJ. Left out:
jet_server-vm and ohkami-tokio, the same code on the Dart VM and on tokio,
each a place from the build here; morojs, 11th, below the ten.

What differs:

- **No Docker.** Their servers run in containers on a bridge network (a
  veth pair and the bridge per packet) with 4 cores against zrk's 12; here
  they run as processes pinned to half the cores, zrk on the other half,
  over loopback. On a 4-vCPU machine that is 2 and 2, so the load
  generator limits the fast servers sooner than theirs does, as it already
  does there. A `--docker` mode would need an image per server and is not
  here; their own harness (`bundle exec rake config`, then the generated
  Makefiles) runs the exact setup.
- **Threads.** Each server gets as many threads or processes as it has
  CPUs, as elsewhere here. In their containers most see 4 CPUs, but
  Caprese (Nim's CPU count) and jet_server (Dart's) count the host's 16;
  here Caprese counts the machine's, as there.
- **io_uring.** Docker's default seccomp profile refuses `io_uring_setup`,
  `io_uring_enter` and `io_uring_register` ([Docker's seccomp
  docs](https://docs.docker.com/engine/security/seccomp/),
  [moby#46762](https://github.com/moby/moby/pull/46762)), so in their
  container Wisp falls back to an epoll per worker. A faithful run uses epoll:
  the suite starts Wisp with `WISP_IO=epoll` unless `WISP_IO` is set, and
  `WISP_IO=uring` measures what a run outside Docker gets.
- **Durations** in the Linux test script: 2 s a route after a 2 s warmup,
  for Wisp, the fast group and their top ten (`--group fast,top`), to keep
  it near 7 minutes. Run it with the defaults for their 15 s and 5 s.

## Real traffic suite

Peak closed-loop req/s is how fast a server goes with 64 connections that
never pause. Real traffic is many more connections, each mostly idle, apps
that wait on databases, take JSON and uploads, hold WebSockets, meet bad
clients and get restarted. `--suite real` measures that on Wisp and every
popular or fast server (on Windows, those that run there), each started
once (`--tests` picks some). They run in the order users, churn, slow,
ws, wait, echo, list, static, upload, abuse, soak, shutdown: the
WebSockets' memory is read before uploads swell a garbage-collected heap.
Three tables come out, traffic, app work and robustness, with Wisp's rank
on every figure.

Traffic, on `/page`, `/json` and `/fortunes`, which every server answers:

- **Users**: each user is a keep-alive connection sending a request every
  `--think` seconds (1 by default, ±50%, uniform), whether or not the last
  answer was late: `/page` half the time, `/json` 30%, `/fortunes` 20%.
  Latency runs from when each request was due, so a server that falls
  behind is charged for the wait (no coordinated omission). From 500 users,
  doubling while a step passes, then two bisections (about 1.19x apart): a
  step is 2 s for new users to connect (spread over a second), then `-d`
  seconds measured, and passes when p99 is within 100 ms (`--slo-ms`), at
  most 0.1% of requests fail and at least 99% of those due are answered.
  Reported: the most users that passed, p99, the server's CPU % (of its
  CPUs) and memory there, KB per user (memory over idle, per user: what
  holding open WebSockets or idle browsers costs), then twice as many
  users, to see whether it degrades (still serves most, slower) or
  collapses. A user at one request a second is a heavy one: a visitor
  loading a page every 10 to 30 s puts 10 to 30 times fewer requests on
  it, so the real count of such visitors is that many times higher. If the
  load generator itself sends more than 10 ms late (p99), the ramp stops
  and the twice-as-many step is skipped; if no step had failed by then,
  the server serves at least the most that passed ("load-limited"), else
  its own limit was found anyway.
- **Churn**: 64 closed loops on `/json`, each request on a new connection
  with `connection: close`, as clients without keep-alive, health checks
  and some proxies send: connections a second, and CPU per connection.
- **Slow**: 2,000 slowloris clients each sending a request head a byte
  every 500 ms, beside 64 normal closed-loop connections on `/page`: their
  rate as a share of the same load alone (measured just before), their
  p99, and how many slow clients the server cut off (cutting them is
  good).

App work, on the practice routes above (n/a where a server has none: a
404, or an answer off the contract, when each is checked first):

- **Wait**: 1,000 closed-loop connections on `/wait`: 50,000 req/s is
  ideal, less means a waiting handler holds up others. req/s, p50, p99.
  On Windows a 20 ms timer fires on the 15.6 ms clock tick, after about
  31 ms, so about 32,000 is ideal there.
- **Echo**: 64 connections posting a valid 170-byte body to `/echo`, after
  an invalid one must get a 4xx: req/s and CPU per request.
- **Upload**: 32 connections posting 1 MiB to `/upload`: MB/s and the most
  memory seen (sampled every 100 ms), after a 9 MiB body must get a 413 or
  a closed connection, and `/json` must still answer.
- **List**: 64 connections on `/list` (1,000 rows of JSON): req/s, CPU.
- **Static**: 64 connections on `/static/app.js` (100,253 bytes, checked against `static/app.js`).
- **WS**: 10,000 idle WebSockets on `/ws` (KB each, from memory), then 64
  of them echoing a 32-byte text message closed loop: messages/s and p99.

Robustness:

- **Abuse**: a garbage request line, a 64 KB header, a 1 MB header, a bad
  chunked body and a request without a host, each on its own connection
  with 2 s for an answer; a 4xx or a close handles it (and any answer to
  the 64 KB header, which is within some servers' limits). Then `/json`
  must answer 200 within a second. "n/5 handled", and what failed.
- **Soak**: users at half the most that passed (1,000 if the users test did
  not run) for `--soak` seconds (60; 0 skips), memory sampled every 5 s:
  first against last, "grows" past 20%.
- **Shutdown** (Linux): 64 connections on `/wait` for 3 s, and SIGTERM to
  the server a second in, while they are still sending: how many requests
  failed (0 is graceful; a connect refused once the server stopped
  listening is expected, not counted), and ms until it exited (killed at
  10 s). Last, since it stops the server.

Server CPU is counted in cycles on Windows: its CPU time is charged per
15.6 ms clock tick to whatever thread is running then, so a server that
wakes briefly between ticks, as under a light load, showed next to none.

The load runs on a tokio runtime per load CPU; on Linux its connections
come from 127.0.0.2 to 127.0.0.255 so that 50,000 of them do not run out of
ports, and the runner raises its open-file limit to the hard limit (warning
and capping the users if that is under twice the users plus 1,000).

```
cargo run -r -p bench-run -- --suite real [-d 10] [--think 1.0]
    [--max-users 50000] [--slo-ms 100] [--soak 60] [--only wisp,fastify]
    [--tests users,churn,slow,wait,echo,upload,list,static,ws,abuse,soak,shutdown]
    [--csv FILE]
```

`--csv` appends a line per server, test and figure.

## Results on Linux

2026-09-28. Ubuntu 24.04 (Linux 6.8) on a 4-vCPU AMD EPYC 7B13 VPS: servers
on CPUs 0-1, load on 2-3. Rust 1.98, Go 1.26.3, Node 26.10. 64 connections,
5 s warmup, 10 s measured, mean of 2 rounds. ASP.NET Core did not start (the
runtime was not on its path); the script that runs this has been fixed since.

| Server      | Path       |  req/s | p50      | p99       | CPU µs/req | Kernel µs | Peak MB |
|-------------|------------|-------:|----------|-----------|-----------:|----------:|--------:|
| Wisp        | /plaintext | 93,476 | 0.62 ms  | 1.50 ms   |       19.8 |      17.8 |       3 |
| Wisp        | /fortunes  | 97,502 | 0.60 ms  | 1.42 ms   |       19.8 |      16.8 |       3 |
| Actix Web   | /plaintext | 94,441 | 0.62 ms  | 1.38 ms   |       20.6 |      17.0 |       5 |
| Actix Web   | /fortunes  | 86,169 | 0.70 ms  | 1.39 ms   |       23.0 |      17.6 |       5 |
| Axum        | /plaintext | 85,776 | 0.71 ms  | 1.60 ms   |       22.9 |      17.5 |       6 |
| Axum        | /fortunes  | 76,818 | 0.79 ms  | 1.94 ms   |       25.6 |      18.1 |       6 |
| Go net/http | /plaintext | 44,404 | 1.17 ms  | 5.44 ms   |       44.7 |      23.3 |      19 |
| Go net/http | /fortunes  | 15,757 | 3.46 ms  | 14.34 ms  |      129.3 |      32.9 |      19 |
| Fiber       | /plaintext | 77,082 | 0.70 ms  | 2.85 ms   |       25.1 |      19.4 |      13 |
| Fiber       | /fortunes  | 22,148 | 2.43 ms  | 9.73 ms   |       90.3 |      25.3 |      19 |
| Fastify     | /plaintext | 40,757 | 1.42 ms  | 4.06 ms   |       49.6 |      23.0 |     238 |
| Fastify     | /fortunes  | 24,576 | 2.34 ms  | 6.53 ms   |       82.0 |      29.9 |     248 |
| SvelteKit   | /plaintext | 10,934 | 4.93 ms  | 19.84 ms  |      186.3 |      35.7 |     444 |
| SvelteKit   | /fortunes  |  2,676 | 17.66 ms | 89.09 ms  |      758.5 |      62.9 |     494 |
| Next.js     | /plaintext |  2,102 | 24.32 ms | 114.69 ms |      971.1 |      94.0 |     665 |
| Next.js     | /fortunes  |    393 | 175.10 ms | 425.98 ms |    5847.7 |     279.1 |     732 |

On a virtual machine the kernel's TCP stack is most of every request (17 of
Wisp's 20 µs), so the fast servers bunch together on plaintext. Fortunes
separates them: Wisp renders the page at the cost of plaintext, 13% more
requests than Actix, 27% more than Axum and 4.4× Fiber, with 3 MB of memory.

**Why Linux has no I/O code of its own.** Three plaintext servers were
measured the same way, to see what tokio leaves on the table (µs of CPU per
request, mean of 2 rounds; run-to-run noise was about 10%):

| Waiting for the next request                     | CPU  | Kernel |
|--------------------------------------------------|-----:|-------:|
| Wisp (tokio, a runtime per core)                 | 19.8 |   17.8 |
| epoll, reading until `EAGAIN`                    | 22.3 |   20.4 |
| epoll, stopping after a short read               | 20.1 |   18.7 |
| io_uring (single issuer, recv and send queued)   | 20.2 |   19.7 |

None beat tokio then. The io_uring prototype waited in `io_uring_enter` for
each completion, with no deferred task work, and rearmed a receive per
request; Wisp's Linux workers now use a driver without those costs
(`crates/wisp/src/uring.rs`, see https://wispweb.dev/docs/design), and `WISP_IO=epoll` runs
the same design on epoll (`crates/wisp/src/epoll.rs`).

## Results on Windows

2026-09-28. Windows 10, AMD Ryzen 7 7800X3D (8 cores, 16 threads). Servers
on logical CPUs 0-7, load on 8-15. Rust 1.97, .NET 10.0.302, Go 1.26.3, Node
26.1. 64 connections, 5 s warmup, 10 s measured. Kernel µs is the part of
the CPU per request spent in the OS, mostly its TCP stack.

| Server       | Path             |   req/s | p50     | p99      | p99.9     | CPU µs/req | Kernel µs |
|--------------|------------------|--------:|---------|----------|-----------|-----------:|----------:|
| Wisp         | /plaintext       | 727,123 | 0.06 ms | 0.42 ms  | 4.22 ms   |        7.8 |       5.7 |
| Wisp         | /fortunes        | 691,036 | 0.07 ms | 0.40 ms  | 5.38 ms   |        8.3 |       5.7 |
| ASP.NET Core | /plaintext       | 659,405 | 0.09 ms | 0.33 ms  | 0.62 ms   |       10.3 |       3.4 |
| ASP.NET Core | /fortunes        | 315,385 | 0.19 ms | 0.46 ms  | 0.88 ms   |       24.1 |       5.8 |
| ASP.NET Core | /fortunes-blazor | 168,849 | 0.35 ms | 0.67 ms  | 2.69 ms   |       45.6 |       7.3 |
| Actix Web    | /plaintext       | 711,909 | 0.08 ms | 0.34 ms  | 1.31 ms   |        9.3 |       5.8 |
| Actix Web    | /fortunes        | 651,935 | 0.08 ms | 0.38 ms  | 3.39 ms   |       10.2 |       5.6 |
| Axum         | /plaintext       | 350,651 | 0.17 ms | 0.37 ms  | 0.51 ms   |        9.9 |       5.6 |
| Axum         | /fortunes        | 334,255 | 0.18 ms | 0.38 ms  | 0.53 ms   |       11.9 |       6.0 |
| Go net/http  | /plaintext       | 497,045 | 0.09 ms | 0.77 ms  | 1.18 ms   |       12.4 |       4.1 |
| Go net/http  | /fortunes        | 135,953 | 0.05 ms | 4.99 ms  | 8.70 ms   |       52.4 |       8.9 |
| Fiber        | /plaintext       | 792,940 | 0.07 ms | 0.19 ms  | 0.82 ms   |        7.1 |       3.5 |
| Fiber        | /fortunes        | 164,097 | 0.30 ms | 1.86 ms  | 3.84 ms   |       44.3 |       8.7 |
| Fastify      | /plaintext       | 347,856 | 0.17 ms | 0.41 ms  | 0.59 ms   |       22.5 |       8.1 |
| Fastify      | /fortunes        | 206,778 | 0.37 ms | 0.61 ms  | 0.80 ms   |       37.9 |      10.5 |
| SvelteKit    | /plaintext       |  75,647 | 0.53 ms | 3.65 ms  | 7.30 ms   |      105.0 |      16.8 |
| SvelteKit    | /fortunes        |  24,907 | 2.02 ms | 9.47 ms  | 13.82 ms  |      320.3 |      23.6 |
| Next.js      | /plaintext       |  13,265 | 1.92 ms | 18.43 ms | 23.55 ms  |      601.3 |      29.1 |
| Next.js      | /fortunes        |   3,867 | 4.48 ms | 88.06 ms | 100.35 ms |     2060.1 |      48.6 |

Wisp's rows come from a second run the same day, after its Windows I/O code
was taken out (below); Actix and Fiber, measured again in that run, came
within 5% of their rows here.

On fortunes, the test that renders a page, Wisp has the lowest CPU per
request: it serves 1.06× Actix, 2.2× Razor Pages and 4.2× Fiber, whose
`html/template` walks the data by reflection. Its templates compile to Rust,
so rendering the page adds about half a microsecond to plaintext's cost. On
plaintext Fiber uses the least CPU, 6.7 to 7.1 µs to Wisp's 7.8, because
tokio waits for sockets on Windows the slow way (below). Wisp's peak memory
was 6 MB.

Run-to-run noise is up to 10%: repeat a run (`--rounds 3`) before trusting
a small gap.

**Where a Windows request's CPU went.** On a tokio runtime per core, Wisp's
plaintext cost 7.9 µs, 5.8 of it in the kernel. tokio waits for a socket on
Windows through mio, which asks the kernel for readiness with an AFD poll per
socket, reads in a second call, and after each request re-arms a poll that
the always writable socket completes at once. A plaintext server on bare mio
measured the pieces, then the same server on I/O completion ports, where a
receive is posted with its buffer and completes with the data (µs per
request, three runs each):

| Waiting for the next request                            | CPU  | Kernel | User |
|---------------------------------------------------------|-----:|-------:|-----:|
| mio: reads and writes, read until `WouldBlock` (tokio)  | 7.26 |   5.65 | 1.60 |
| mio: reads only, read until `WouldBlock`                | 5.98 |   4.45 | 1.53 |
| mio: reads only, re-arm the poll after a read           | 5.43 |   4.13 | 1.31 |
| completion port: overlapped `WSARecv`, blocking `send`  | 4.02 |   3.55 | 0.47 |
| completion port: overlapped `WSARecv` and `WSASend`     | 3.72 |   3.29 | 0.43 |
| Registered I/O (RIO): registered buffers, polled queue  | 2.67 |   2.26 | 0.41 |

For a while Wisp had Windows workers of its own on the fifth row, the way
Go, .NET and libuv drive Windows sockets, which took its plaintext to 4.6 µs
and fortunes to 5.2. They needed `unsafe`, and Windows is where Wisp apps are
developed, not where they are served, so they were taken out: Wisp is on
tokio everywhere, and has no `unsafe` code.

**Why thread per core.** With one multi-threaded tokio runtime, plaintext
topped out at 418k req/s with 3.4 of 8 cores busy: every socket event goes
through one I/O driver. With a runtime per core the same code scaled
linearly (190k, 422k, 715k req/s at 1, 2, 4 threads) until the load
generator became the limit. Axum is that first shape (`axum::serve` on one
runtime), which is why it has cores to spare at half the throughput.

## Tokens

What an app costs to write, since AI writes most of it: `bench/tokens/apps`
has the same five features in each stack, written idiomatically and as
short as each allows, and `cargo run -p wisp-tokens` counts them. A list
page loading its data, a contact form (name 1 to 50 characters, a valid
email; a 422 that shows each problem and keeps what was typed, else a
redirect), a JSON endpoint of the list, a layout with a nav, and a live
search filtered in the browser; `data` is the list's type and source,
`setup` the dependencies and wiring a stack needs. Only hand-written files
count, with their paths; what `wisp new`, `sv create`, `create-next-app` or
`cargo new` writes does not. The Wisp app is a workspace member, with tests
that each feature works.

| Stack | list | form | api | layout | search | data | setup | total | chars / 4 | files |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| **Wisp** | 58 | 89 | 31 | 60 | 111 | 115 | 0 | **464** | 283 | 6 |
| SvelteKit | 128 | 470 | 57 | 83 | 192 | 72 | 0 | 1002 | 644 | 9 |
| Next.js | 107 | 439 | 44 | 108 | 241 | 71 | 0 | 1010 | 725 | 8 |
| Axum + askama | 145 | 553 | 29 | 104 | 217 | 123 | 285 | 1456 | 1020 | 7 |
| Actix + tera | 164 | 565 | 46 | 104 | 237 | 123 | 292 | 1531 | 1061 | 7 |

SvelteKit and Next.js take 2.2x Wisp's tokens, Axum and Actix 3.1x and
3.3x. The estimate and what changed to get here:
[the tokens page](https://wispweb.dev/docs/tokens).

## Caveats

- Loopback on one machine, with the load generator on the same machine. The
  Linux machine is a small VPS, where the kernel is most of every request.
- On Windows the load generator tops out near 830k req/s, so the fastest
  servers were not at 100% CPU: compare CPU per request at the top, not
  req/s.
- No database. Fortunes measures routing, rendering, escaping and HTTP.
- Response sizes differ by default: Wisp's page links its 6 KB script,
  SvelteKit's and Next.js's pages carry hydration data and scripts.
- Bun is not included: on Windows it cannot share a port between processes,
  so it would serve from one CPU.
- Kestrel sends `/plaintext` chunked (the minimal API default for a returned
  string), so its responses are 35 bytes larger.
