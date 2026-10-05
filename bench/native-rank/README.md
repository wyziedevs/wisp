# Native ranking bench

Wisp's native server against eleven popular web frameworks, same routes, same machine, same
load, run twice. Results and ranks: `results/report.md`; raw numbers: `results/pass1/*.json`,
`results/pass2/*.json`.

Reproduce, on a Linux host with 4+ cores and `oha`, Rust, Node 24 and Bun installed:

```sh
./run.sh all        # setup.sh (Go, .NET, JDK, Maven, venv), build, verify, pass 1, pass 2, report
```

or step by step: `./run.sh build | verify | pass 1 | pass 2 | report`.

## The 12, and why

| # | Framework | Runtime | Why |
|---|---|---|---|
| 1 | Wisp | Rust, own epoll/io_uring server | the subject |
| 2 | ASP.NET Core minimal API | .NET 10, Kestrel | the .NET default; Release, Server GC on, concurrent GC off, TieredPGO (TechEmpower's published settings) |
| 3 | Axum | Rust, tokio + hyper | the most used Rust framework |
| 4 | Actix Web | Rust, actix-rt | TechEmpower's Rust leader |
| 5 | Gin | Go 1.27 | the most used Go framework (over net/http and Fiber) |
| 6 | Fastify | Node 24, cluster | the fast Node framework |
| 7 | Express | Node 24, cluster | the most used Node framework |
| 8 | Hono | Bun | the Bun entry (over Elysia: Hono is the more used) |
| 9 | Spring Boot (MVC, Tomcat) | JDK 21 | the most used JVM framework (over Quarkus) |
| 10 | FastAPI | Python 3.12, uvicorn (uvloop, httptools) | the most used Python API framework (over Django) |
| 11 | Next.js 15 | Node 24, standalone, cluster | JS full-stack |
| 12 | SvelteKit | Node 24, adapter-node, cluster | JS full-stack |

Left out to land on twelve: Laravel, Rails, Phoenix, Django, Quarkus, Elysia, Fiber. Each is a
language or stack already represented by a neighbour above; Next.js and SvelteKit were
requested by name.

## Routes (identical bytes, checked by `run.mjs verify`)

| Route | Response |
|---|---|
| `GET /` | `Hello, World!` (`text/plain; charset=utf-8`) |
| `GET /json` | `{"message":"Hello, World!"}` |
| `GET /params/42?q=hello` with `Cookie: sid=abc123; theme=dark` | `id=42 q=hello sid=abc123` (text) |
| `GET /list` | HTML, 1000 items `Item <N> & co`, each escaped by the framework's own escaper |
| `GET /json-big` | JSON array of 200 objects `{id,name,active,score,tags}`, compact (14,234 bytes) |

`/list` is built the way each framework's docs would: Wisp, Next.js and SvelteKit components,
Go `html/template`, FastAPI Jinja2, and a string builder with the framework's HTML escaper where
there is no engine in the default stack (Axum, Actix, ASP.NET Core, Spring, Fastify, Express,
Hono). Axum and Actix write the escaped text as a literal around the number (`Item &lt;{i}&gt;
&amp; co`, no escaper call and no string per item); the Wisp template does the same
(`{#each 1..=1000 as i}<li>Item &lt;{i}&gt; &amp; co</li>{/each}`), so the three do the same work.
Until 2026-10-05 Wisp's page built 1000 `format!` strings and escaped each, which was most of its
257 us against 109 and 116. `verify` compares the 1000 `<li>` texts, not the wrapper: Wisp adds its head
(build id and client script), Next.js its RSC payload (153 KB), SvelteKit with `csr = false` sends
neither. Everything else (status, body bytes, length) is compared exactly; the content-type
differs only in spelling (`;charset=` spacing and case).

## Method

- Host: the 4-core Ubuntu 24.04 VPS (also used by other jobs, so every measurement runs under
  `flock /tmp/wisp-bench.lock`). Server pinned to cores 0-1 (`taskset`), `oha` 1.16 pinned to
  cores 2-3, 64 connections, keep-alive, 10 s runs.
- Per framework and route: 5 s warm-up, then 5 timed runs; the median (by req/s) is the cell.
  A server is started fresh for each framework, killed afterwards; port 18480.
- Hypervisor steal: the VPS is a shared VM and its steal time swung between 0.4% and 55% during
  these runs, which moves every number by up to 10x. `run.mjs` reads `/proc/stat` around every
  timed run, redoes a run once if steal was above 8% (`STEAL_MAX`, `STEAL_TRIES`), and otherwise
  keeps the quietest try and marks the cell `~` (noisy). Steal and discards are in the JSON.
  The first two complete passes ran without this gate (`results/ungated/`, steal 45-60% for most
  of pass 1) and are kept only as evidence of how much the noise matters; `results/strict-partial/`
  is a pass abandoned at a 5% gate because retries then dominated the run time.
- Cold start: spawn to the first `200` on `/`, polled every 5 ms. RSS: the sum over every
  process of the server (workers, JVM, Bun processes) after all five routes were loaded.
- Production mode and no logging everywhere. Multi-core: Node frameworks run `cluster` with one
  worker per available core, Bun runs one process per core with `reusePort`, uvicorn `--workers 2`,
  Spring Boot `-XX:+UseParallelGC -Xms1g -Xmx1g`, Go, Tokio, Actix and Kestrel use their defaults
  (which see two cores). Express has ETag off and `x-powered-by` off, Gin has no logger or
  recovery middleware, FastAPI has no docs routes.
- Two independent passes at different times; pass 2 walks the frameworks in reverse order. Cells
  where no two clean passes are within 5% are flagged in the report; re-runs (passes 3 to 5) were
  added until time ran out, and the cells that still disagree are listed with the reason (steal).
  Ranks use the median of the clean passes.
- Wisp is built `--release` with its own `Cargo.toml` profile (fat LTO, one codegen unit); Axum
  and Actix get the same profile, plus `panic = "abort"`.

Not covered: pipelining, other concurrency levels, bodies, TLS, a database. Throughput ranks here
move by 10 to 20% between runs on a shared host (see `results/report.md` for the observed gaps);
treat neighbours as ties.

## Results (full tables: `results/report.md`)

Five passes on the shared 4-core VPS (see Hypervisor steal above): 1 and 2 are the two required
independent passes (pass 2 in reverse order, hours later), 3 is a partial tie-break (strict
steal gate, 5 of 12 servers), 4 and 5 re-ran everything and also record CPU per request, which does
not move with steal. Req/s is the median of the clean passes; ranks:

| Framework | `/` | `/json` | `/params` | `/list` | `/json-big` | CPU per request, rank sum |
|---|---|---|---|---|---|---|
| Wisp | 1 | 1 | 3 | 3 | 1 | 8 (best) |
| Actix Web | 2 | 3 | 1 | 1 | 2 | 9 |
| Axum | 3 | 2 | 2 | 2 | 3 | 13 |
| ASP.NET Core | 4 | 4 | 4 | 5 | 4 | 20 |
| Hono (Bun) | 5 | 5 | 6 | 9 | 6 | 31 |
| Go Gin | 6 | 6 | 5 | 10 | 7 | 31 |
| Fastify | 7 | 7 | 8 | 6 | 5 | 33 |
| Express | 8 | 9 | 7 | 7 | 9 | 40 |
| Spring Boot | 10 | 8 | 9 | 4 | 8 | 42 |
| FastAPI | 9 | 11 | 10 | 8 | 12 | 51 |
| SvelteKit | 11 | 10 | 11 | 11 | 10 | 53 |
| Next.js | 12 | 12 | 12 | 12 | 11 | 59 |

Plainly:

- Wisp is top 3 in every route, but the top three (Wisp, Actix Web, Axum) are within the noise of
  each other on `/`, `/json`, `/params` and `/json-big`: their order flips between passes (Wisp
  was 4th in pass 1 `/`, 1st in pass 2). Do not read a win from those ranks; read "top tier".
- `/list` was Wisp's weak cell in these passes: 3rd in all five, 257 us against Axum's 109 and
  Actix's 116, because its page built and escaped 1000 strings the others did not (see above).
  With the same work (2026-10-05, callgrind, one request): Wisp 153k instructions, Axum 370k,
  Actix 425k; CPU per request in three alternating c=64 rounds: Wisp 108/92/172 us, Axum
  139/168/180, Actix 179/199/229. The passes above predate it. Gin's `html/template` is the slowest of the compiled
  stacks here (a stock-library choice, not a tuned one); Next.js renders 1000 React elements plus
  its RSC payload.
- CPU per request (steal-proof): Wisp is first on `/`, `/json` and `/params` (19 to 23 us), second
  on `/json-big` (130 us against Actix 128), third on `/list`.
- Memory after load: Wisp 4 MB, Actix 5, Axum 6, Gin 24, Hono 85, ASP.NET Core 95, FastAPI 140,
  Fastify 376, Express 492, Spring 568, Next.js 1.1 GB, SvelteKit 1.8 GB. Cold start to first 200:
  Actix 25 ms, Axum 27, Wisp 33, Gin 52, Hono 89, SvelteKit 399, Express 464, Fastify 566, ASP.NET
  Core 641, FastAPI 1.2 s, Next.js 1.5 s, Spring Boot 10.7 s (two cores, JIT and classpath scan).
- Agreement: most throughput cells never got two clean passes within 5% (list in the report),
  because the host's steal time was above the gate in most runs of most passes. Those cells are
  provisional by the report's own flag; the ranks above are stable where the gaps are large (the
  tail of the table moves by at most one place) and not where they are small (the top three).
