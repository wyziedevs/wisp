# Wisp on the TechEmpower plaintext and JSON tests

Wisp's `/plaintext` and `/json` against the TechEmpower Framework Benchmarks (TFB) reference
sources of Axum, Actix Web, Express, Fastify and Hono, measured with TFB's own wrk scripts on one
shared 4-vCPU VM. Every measured number is published. This is **not** a
TechEmpower result and is not comparable with their published rounds (see Caveats).

This file is generated: `python3 aggregate.py` fills the tables from `raw/`. Raw wrk output is in
`raw/<contender>/`, summary data in `results.json`, everything reproduced by `run.sh`.

## Summary

Derived by `aggregate.py` from `results.json`; nothing here is hand-written. Medians of 3 runs,
15 s each, server on 2 pinned cores.

{{HEADLINE}}

## What TFB specifies (read from the source, not from memory)

Spec: [Framework Tests Overview](https://github.com/TechEmpower/FrameworkBenchmarks/wiki/Project-Information-Framework-Tests-Overview)
(sections "JSON Serialization" and "Plaintext").

| | JSON serialization | Plaintext |
|---|---|---|
| URI | recommended `/json` | recommended `/plaintext` |
| Status | `200 OK` | `200 OK` |
| Content-Type | `application/json` | `text/plain` |
| Headers | `Content-Length` or `Transfer-Encoding`; `Server` and `Date` | same |
| Body | `{"message":"Hello, World!"}` | `Hello, World!` |
| Other | no gzip; keep-alive strongly encouraged; GET | no gzip; "Server support for HTTP/1.1 pipelining is assumed"; GET |
| Concurrency | 16 to 512 (toolset default `16 32 64 128 256 512`) | 256, 1024, 4096, 16384 |

Toolset (`toolset/wrk/` and `toolset/run-tests.py` in the TFB repo, commit
`57d92fbec6f8fd7431bc77326dd0484e60c96e20`, 2026-03-23): `--duration` default 15 s; per test a
primer of `wrk ... -d 5 -c 8 --timeout 8 -t 8`, sleep 5, a warmup of the full duration at the
maximum concurrency with `-t $(nproc)`, sleep 5, then each level with
`wrk -H 'Host: <server>' -H 'Accept: <accept>' -H 'Connection: keep-alive' --latency -d 15 -c <c> --timeout 8 -t <min(c, nproc)> <url>`,
sleep 2. Plaintext adds `-s pipeline.lua -- 16` (`pipeline.lua` is included verbatim: it repeats
the request 16 times per write). The Accept headers are TFB's, copied from
`toolset/test_types/abstract_test_type.py` into `run.sh`.

## Method

- One VM runs both sides. The server is pinned to cores `0-1` and wrk to cores `2-3` with
  `taskset`; the two never share a core. wrk runs with `-t 2` (`min(c, client cores)`, as TFB's
  `min(c, nproc)` with the client's cores). Loopback, no Docker.
- Before every measurement `run.sh` waits for the VM to be quiet, starts the server, and checks it
  the way TFB's verifier does (status, content type, length header, `Server`, `Date` present and
  changing, no gzip, exact body, 16 pipelined requests answered 16 times). A contender that fails
  is omitted. Verification output is in `raw/<contender>/verify.txt` with its process, thread
  and CPU-affinity counts.
- Each level is measured 3 times back to back after one primer and one warmup per workload. Tables
  show the median, min and max of the 3 requests-per-second figures, and the median of the 3
  latency averages and p99s.
- Each run also records CPU used by anything other than the server and wrk ("foreign", from
  `/proc`); a run above 5% would be kept as `*.tainted*` and repeated. Hypervisor *steal* time is
  recorded too (last line of each file) but cannot be avoided.
- Every contender uses its recommended production settings (`NODE_ENV=production`, release builds
  with LTO). Wisp has no tuning: `cargo build --release`, no environment knobs, `PORT=8080`.
- **wrk's p99 prints as `0.00us` on pipelined runs** (a wrk quirk with pipelining); those cells
  show `n/a`. TFB's published plaintext tables report throughput only for the same reason.

## Contenders and sources

| Contender | Source | Notes |
|---|---|---|
| Wisp | `wisp/` (this repo) | two route files; `Server` header set through `[package.metadata.wisp] headers`, Wisp's documented static-header config, because TFB requires the header and Wisp does not send one by default |
| Axum | TFB `frameworks/Rust/axum` (`axum` test: `src/main.rs`, `src/server.rs`) | database bins, features and dependencies removed; hyper-based server, mimalloc, one current-thread runtime per core, `RUSTFLAGS="-C target-cpu=native"` as in TFB's dockerfile |
| Actix Web | TFB `frameworks/Rust/actix` (`tfb-web`, the default test) | database code removed; snmalloc, `-C target-cpu=native` as TFB's dockerfile |
| Express | TFB `frameworks/JavaScript/express` `app.js` + `src/utils.mjs` | cluster, fast-json-stringify |
| Fastify | TFB `frameworks/JavaScript/fastify` `app.js` + `create-server.js` | cluster, response schema |
| Hono on Node | TFB `frameworks/JavaScript/hono` (`src/clustered.js`, `server.js`) with TFB's own lockfile | Hono 3.12 and `@hono/node-server` 1.10 (TFB pins these) |
| Hono on Bun | `hono-bun/` | **not TFB source**: TFB has no Bun entry. Same routes as the Node entry, `Bun.serve` with `reusePort`, one process per core |
| SvelteKit | `sveltekit/` | **not TFB**: two plain `+server.js` route handlers, `adapter-node` production build, one process per core through `node:cluster` |
| Next.js | `next/` | **not TFB**: two App Router route handlers, `output: 'standalone'`, one process per core through `node:cluster` |
| Nuxt | `nuxt/` | **not TFB**: two Nitro server routes (`server/routes/*.js`), `nuxt build` with the `node-server` preset, one process per core through `node:cluster` |

Deviations from TFB source, all of them: database parts removed; Express and Fastify use
`os.availableParallelism()` instead of `os.cpus().length` (the latter ignores `taskset` and would start
4 workers on 2 cores; marked in the code); one Node version (24) for all JavaScript entries
(TFB's dockerfiles use 20 and 24); Wisp is built without `target-cpu=native`, which gives the Rust
contenders a small edge Wisp does not get.

Allocators, LTO and wire details, as built:

- Axum: mimalloc, `lto = "fat"`, `codegen-units = 1`, `target-cpu=native`.
- Actix: snmalloc, `lto = true` (thin-local LTO, not fat), `codegen-units = 1`, `target-cpu=native`; Actix sends `Server: A` (TFB's own value).
- Wisp: system allocator, `lto = "fat"`, `codegen-units = 1`, no `target-cpu=native`.
- `Content-Type` is the bare TFB value on every contender: `text/plain` for `/plaintext`, `application/json` for `/json`
  (Wisp, Axum and the Hono entries set it explicitly; their framework defaults add `; charset=utf-8`).
- wrk and the server share one VM on separate pinned cores (server `0-1`, client `2-3`).
- `hono-bun` pins `hono` 3.12.12 exactly in `package.json`; there is no `bun.lock` because bun was not available to generate one.

Wisp on this VM: `io_uring` is refused by the kernel (`registering the buffer ring: Invalid
argument`), so Wisp runs on its `epoll` driver. That is Wisp's own fallback, not a setting.

## Environment

```
{{ENV}}```

Two shared-VM facts matter: `scaling_governor` is not exposed (frequency is the host's choice), and
the VM is a tenant of a shared host, so steal time and neighbours move results from one moment to the
next. Details in Caveats.

## Results

All contenders, both workloads and every connection level, with every metric. Sort key within each
table: median requests per second, descending; the Wisp rows are bold. Source: `raw/` wrk output
(`results.json`), run date and hardware in Environment above. Rows marked "(supplementary)" are not
part of the summary. "steal % max" is the highest hypervisor steal share of CPU over the runs. "errors" counts non-2xx responses and wrk socket errors summed over the 3 runs.
Contenders that stop answering show `0`; that is a measured result of this setup.

{{TABLES}}

## Noise: the same binary at different moments

`./run.sh noise` alternates Wisp (defaults) and the supplementary build (identical code, only
`WISP_MAX_CONNS` differs, which changes nothing at 16 or 256 connections) for 3 rounds, JSON only:

{{NOISE}}

The spread of one binary against itself is the floor for what any of these tables can tell apart.

## Reference from TechEmpower

| Framework | Latest published round | Plaintext | JSON | Source |
|---|---|---|---|---|
| Axum | Round 23 (2025-02-24) | not retrievable | not retrievable | see below |
| Actix Web | Round 23 | not retrievable | not retrievable | see below |
| Fastify | Round 23 | not retrievable | not retrievable | see below |
| Express | Round 23 | not retrievable | not retrievable | see below |
| Wisp | not listed | - | - | - |

Round 23 is the latest: the project's own site lists it last
([techempower.com/benchmarks](https://www.techempower.com/benchmarks/), results run
[91a66052-9d86-446c-b31a-eadbd669ed08](https://tfb-status.techempower.com/results/91a66052-9d86-446c-b31a-eadbd669ed08),
announcement [#9589](https://github.com/TechEmpower/FrameworkBenchmarks/issues/9589)), and the
project announced it was being [sunset](https://github.com/TechEmpower/FrameworkBenchmarks/issues/10932)
in 2026. The per-test numbers are not in this table because the results host
`tfb-status.techempower.com` refuses connections (from this machine and from the benchmark VM),
the Wayback Machine holds no copy of the results file, and no secondary source found quotes
all four frameworks' plaintext and JSON figures for the same run. Nothing was filled in from
memory. Round 23 ran on three physical servers with 40-gigabit networking (not this VM), so even
with the numbers they would be a different scale from the tables above.

## Caveats

- **Shared VM, not TFB hardware.** A 4-vCPU AMD EPYC 7B13 VM; the server gets 2 cores and wrk gets
  2. TFB runs the server and the load generator on separate physical machines over a fast
  network, so absolute numbers here are not comparable to any TechEmpower round.
- **Noise.** Steal time and neighbours (see the noise table). Differences inside the min-max ranges are ties.
- **Loopback with the client on the same VM**: wrk competes with the kernel's TCP work for the
  same host; wrk can be the limit for the slowest servers' latency numbers and for 16384
  connections (2 threads).
- **Plaintext at 16384 connections** is beyond what this setup serves for most contenders (wrk
  timeouts after 8 s; a `0` median means at least 2 of 3 runs completed no request). Wisp's default
  `WISP_MAX_CONNS=10000` answers `503` past that many connections.
  A cell at 16384 that records `0`, no valid run or mostly non-2xx replies is labelled "load-generator
  limited (local port range)": the VM has about 28k local ports (32768-60999), Linux searches half of
  them for every `connect()`, and wrk's event loop blocks while refused clients reconnect. That is a
  measurement caveat of this setup, not a Wisp failure. Widening the range (see `bench/README.md`) lifts it.
- **Pipelined rows have no p99** (wrk), and latency is queueing under a 16-deep pipeline, not
  request latency.
- **Drain before every run.** Earlier rounds showed Next.js at 0 req/s at every pipelined plaintext
  level. Cause, reproduced on the VPS: wrk closes its sockets after the 16384 x 16 warmup, but Node's
  http server still runs every request already pipelined on them (about 262k), so Next.js stayed at
  100% of both server cores for about 180 s with no client attached and every timed run in that window
  completed nothing; fresh, the same build answered 16 of 16 pipelined requests and about 1,500 req/s at
  256 connections, and again after the backlog drained. `run.sh` now waits, for every contender, until
  the server uses under 5% of a core before each run (`# drain-before` in each raw file). The heap was not
  the cause (8 GB changed nothing), so Next.js runs on Node's default heap like every Node stack.
  SvelteKit, Next.js and Nuxt are plain route handlers, not optimised entries, and not TFB code.
- Single pass per contender, in the order Wisp, Axum, Actix Web, Express, Fastify, Hono (Node), Hono (Bun), SvelteKit, Next.js, Nuxt, then the supplementary build: contenders measured later
  saw a different moment of the host than Wisp did (the noise experiment shows the size of that).
- Wisp was not tuned; contenders were not tuned beyond what their TFB entries or docs recommend.

## Omitted

{{OMITTED}}

Nothing else was left out. Hono on Bun, SvelteKit, Next.js and Nuxt are labelled as non-TFB above.

## Reproduce

On a Linux host with `wrk`, Rust, Node 24 and Bun in `/opt/node`, `/opt/bun`, `~/.cargo`:

```
cd bench/tfb
./run.sh build          # builds and installs every contender
./run.sh verify         # TFB-style response checks only
./run.sh bench          # the full matrix (about 100 minutes), raw output in raw/
./run.sh noise          # the same-binary experiment
python3 aggregate.py    # results.json and this file (from RESULTS.template.md)
```

`SERVER_CPUS`, `CLIENT_CPUS`, `DURATION`, `RUNS`, `WORKLOADS` and `CONTENDERS` are environment
variables. The run in this repository: server `0-1`, client `2-3`, 15 s, 3 runs.
