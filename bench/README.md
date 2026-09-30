# Benchmarks

Wisp against the ten most popular web frameworks and the ten fastest
(TechEmpower's top tier), on the same machine, doing the same work.

- Popular: ASP.NET Core, Actix Web, Axum, Go's net/http, Gin, Fiber,
  Express, Fastify, SvelteKit, Next.js.
- Fastest: Actix Web, may-minihttp, xitca-web, ntex, hyper, fasthttp,
  Vert.x, uWebSockets.js, Bun, Elysia.

```
cargo run -r -p bench-run -- [-c 64] [-d 10] [-w 5] [--rounds 1]
    [--pipeline 1] [--group fast|popular|all] [--only wisp,actix]
    [--paths fortunes] [--no-build] [--csv results.csv]
```

Runs on Linux and Windows. Needs Rust; a server whose toolchain (the .NET 10
SDK, Go, Node, Bun, a JDK and Maven) is missing, or whose build fails, is
skipped with a note. may-minihttp, uWebSockets.js, Bun and Elysia run on
Linux only (may-minihttp's Windows I/O answers a kept-alive connection's
first request again; the others share the port with SO_REUSEPORT).
`--group` picks either list or both (the default), Wisp always included;
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
  actix|axum|may|xitca|ntex|hyper`), all rendering with Askama, which
  compiles templates to Rust as Wisp does (`/page` inherits `templates/`'s
  layout), and serializing with serde. A Cargo workspace of its own, so none
  of it reaches Wisp's `Cargo.lock`.
- `go/`: net/http, Gin, Fiber v3 and bare fasthttp in one binary
  (`bench-go nethttp|gin|fiber|fasthttp`), all rendering with
  `html/template` (Gin's and Fiber's html renderers wrap it; `/page`'s
  layout is a `define`) and serializing with `encoding/json`.
- `java/`: Vert.x 4.5 on epoll, a verticle per event loop, the pages from a
  `StringBuilder` as its TechEmpower entry does. `mvn package`.
- `node/`: Express (its defaults) and Fastify, the pages from template
  literals (`fortunes.mjs`, `page.mjs`: no template engine, the fastest
  path), SvelteKit with adapter-node (`+page.server.js` and `+page.svelte`,
  `/page` in a `(site)` route group with a `+layout.svelte`), Next.js with
  the App Router (server components forced dynamic, so they render per
  request like the rest, not once at build; `/page` under a `(site)` layout),
  and uWebSockets.js. Each runs under
  `cluster.mjs`, one process per server CPU, as `pm2 -i` does. Bun
  (`Bun.serve`) and Elysia use the same page and run under
  `bun-cluster.js`, one process per CPU sharing the port.
- `run/`: the runner, `bench-run`.
- `load/`: the load generator, a library the runner calls and a command of
  its own (`wisp-load http://127.0.0.1:3000/ -c 64 -d 10`). It is
  closed-loop: each connection sends a request and waits for the whole
  response before the next (no pipelining), like wrk and bombardier by
  default; `--pipeline 16` sends 16 at once instead, timing each response
  from the first byte of its batch. It records latency in a log-linear
  histogram and counts only requests that complete in the measured window.

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
runtime (.NET, the JVM, Node and Bun are installed apart): Wisp's binary,
ASP.NET's publish directory, Vert.x's jar, SvelteKit's `build/`, Next.js's
standalone output, the `node_modules` of the others. The Rust and Go servers
share one binary among several frameworks, so they have no size of their
own. It prints the results as a Markdown table, like the one below.

Only sources are tracked here; builds, `node_modules` and lockfiles are
ignored.

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
(`crates/wisp/src/uring.rs`, see docs/design.md), and `WISP_IO=epoll` runs
the tokio path for comparison.

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
