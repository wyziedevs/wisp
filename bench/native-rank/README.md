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
Hono). `verify` compares the 1000 `<li>` texts, not the wrapper: Wisp adds its head
(build id and client script), Next.js its RSC payload (153 KB), SvelteKit with `csr = false` sends
neither. Everything else (status, body bytes, length) is compared exactly; the content-type
differs only in spelling (`;charset=` spacing and case).

## Method

- Host: the 4-core Ubuntu 24.04 VPS (also used by other jobs, so every measurement runs under
  `flock /tmp/wisp-bench.lock`). Server pinned to cores 0-1 (`taskset`), `oha` 1.16 pinned to
  cores 2-3, 64 connections, keep-alive, 10 s runs.
- Per framework and route: 5 s warm-up, then 5 timed runs; the median (by req/s) is the cell.
  A server is started fresh for each framework, killed afterwards; port 18480.
- Cold start: spawn to the first `200` on `/`, polled every 5 ms. RSS: the sum over every
  process of the server (workers, JVM, Bun processes) after all five routes were loaded.
- Production mode and no logging everywhere. Multi-core: Node frameworks run `cluster` with one
  worker per available core, Bun runs one process per core with `reusePort`, uvicorn `--workers 2`,
  Spring Boot `-XX:+UseParallelGC -Xms1g -Xmx1g`, Go, Tokio, Actix and Kestrel use their defaults
  (which see two cores). Express has ETag off and `x-powered-by` off, Gin has no logger or
  recovery middleware, FastAPI has no docs routes.
- Two independent passes at different times; pass 2 walks the frameworks in reverse order. Cells
  where the passes differ by more than 5% are flagged in the report and re-run or explained.
  Ranks use the mean of both passes.
- Wisp is built `--release` with its own `Cargo.toml` profile (fat LTO, one codegen unit); Axum
  and Actix get the same profile, plus `panic = "abort"`.

Not covered: pipelining, other concurrency levels, bodies, TLS, a database. Throughput ranks here
move by 10 to 20% between runs on a shared host (see `results/report.md` for the observed gaps);
treat neighbours as ties.
