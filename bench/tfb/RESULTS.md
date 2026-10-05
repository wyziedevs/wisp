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

Wisp (defaults) has a min-max range above every other contender's at 3 of 9 workload and connection levels; at 4 more its range overlaps that of the highest median (a tie within noise). Counted from the tables below, supplementary row excluded.

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
date: 2026-10-04T06:57:24Z
cpu: AMD EPYC 7B13 64-Core Processor
cpus: 4  server_cpus=0-1 client_cpus=2-3 duration=15 runs=3
MemTotal:        8131752 kB
kernel: 6.8.0-142-generic
PRETTY_NAME="Ubuntu 24.04.5 LTS"
wrk: wrk debian/4.1.0-4build2 [epoll] Copyright (C) 2012 Will Glozer
rustc: rustc 1.98.1 (48a229cea 2026-09-01)
node: v24.21.0
bun: 1.4.2
somaxconn: 4096 tcp_max_syn_backlog: 512 ulimit-n: 1048576
governor: cat: /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor: No such file or directory
wisp rev: 847bf8d42429d83cf713c32b4c261f93c8ca240d
crates (Cargo.lock):
/root/tfb/bench/tfb/axum/Cargo.lock:name = "axum"
	/root/tfb/bench/tfb/axum/Cargo.lock-version = "0.8.7"
/root/tfb/bench/tfb/axum/Cargo.lock:name = "hyper"
	/root/tfb/bench/tfb/axum/Cargo.lock-version = "1.8.1"
/root/tfb/bench/tfb/axum/Cargo.lock:name = "mimalloc"
	/root/tfb/bench/tfb/axum/Cargo.lock-version = "0.1.48"
/root/tfb/bench/tfb/axum/Cargo.lock:name = "tokio"
	/root/tfb/bench/tfb/axum/Cargo.lock-version = "1.48.0"
/root/tfb/bench/tfb/actix/Cargo.lock:name = "actix-http"
	/root/tfb/bench/tfb/actix/Cargo.lock-version = "3.9.0"
/root/tfb/bench/tfb/actix/Cargo.lock:name = "actix-web"
	/root/tfb/bench/tfb/actix/Cargo.lock-version = "4.9.0"
/root/tfb/bench/tfb/actix/Cargo.lock:name = "snmalloc-rs"
	/root/tfb/bench/tfb/actix/Cargo.lock-version = "0.3.8"
/root/tfb/bench/tfb/actix/Cargo.lock:name = "tokio"
	/root/tfb/bench/tfb/actix/Cargo.lock-version = "1.44.2"
express: â”œâ”€â”€ express@5.2.1 â””â”€â”€ fast-json-stringify@6.4.0  
fastify: â””â”€â”€ fastify@5.12.5  
hono-node: â”œâ”€â”€ @hono/node-server@1.10.1 â””â”€â”€ hono@3.12.12  
sveltekit: â”œâ”€â”€ @sveltejs/adapter-node@5.5.7 â”œâ”€â”€ @sveltejs/kit@2.70.3 â”œâ”€â”€ @sveltejs/vite-plugin-svelte@5.1.1 â”œâ”€â”€ svelte@5.57.1 â””â”€â”€ vite@6.4.3  
next: â”œâ”€â”€ next@15.5.27 â”œâ”€â”€ react-dom@19.3.0 â””â”€â”€ react@19.3.0  
hono-bun: /root/tfb/bench/tfb/hono-bun node_modules (1 installed) â””â”€â”€ hono@3.12.12 
```

Two shared-VM facts matter: `scaling_governor` is not exposed (frequency is the host's choice), and
the VM is a tenant of a shared host, so steal time and neighbours move results from one moment to the
next. Details in Caveats.

## Results

All contenders, both workloads and every connection level, with every metric. Sort key within each
table: median requests per second, descending; the Wisp rows are bold. Source: `raw/` wrk output
(`results.json`), run date and hardware in Environment above. Rows marked "(supplementary)" are not
part of the summary. "steal % max" is the highest hypervisor steal share of CPU over the runs. "errors" counts non-2xx responses and wrk socket errors summed over the 3 runs.
Contenders that stop answering show `0`; that is a measured result of this setup.


#### Plaintext (pipeline depth 16), 256 connections

| # | Contender | req/s median | min | max | latency avg (ms) | latency p99 (ms) | errors | steal % max | tied with |
|---:|---|---:|---:|---:|---:|---:|---|---:|---|
| **1** | **Wisp (defaults)** | **1,129,577** | **1,092,693** | **1,171,462** | **1.45** | **n/a** | **-** | **0.8** | **-** |
| **-** | **Wisp, WISP_MAX_CONNS=0 (supplementary)** | **1,116,591** | **960,570** | **1,116,892** | **1.35** | **n/a** | **-** | **1.6** | **-** |
| 2 | Actix Web (TFB source) | 603,163 | 575,108 | 630,192 | 4.25 | 38.10 | - | 2.8 | - |
| 3 | Axum (TFB source) | 276,948 | 245,050 | 332,863 | 9.05 | 50.28 | - | 17.6 | - |
| tie | Fastify (TFB source) | 51,713 | 40,422 | 60,754 | 218 | 3510 | 34 socket | 2.6 | Express |
| tie | Express (TFB source) | 40,421 | 37,511 | 42,852 | 359 | 4850 | - | 3.8 | Fastify |
| tie | Hono on Node (TFB source) | 28,966 | 3,039 | 29,143 | 449 | 5630 | 138 socket | 2.2 | Hono on Bun, SvelteKit |
| tie | SvelteKit (not TFB) | 10,929 | 502 | 11,055 | 472 | 5590 | 144 socket | 1.5 | Hono on Bun, Hono on Node |
| tie | Hono on Bun (not TFB source) | 10,599 | 10,565 | 10,628 | 211 | 410 | - | 1.2 | Hono on Node, SvelteKit |
| - | Next.js (not TFB) | No valid result: no pipelined response completed (heap raised to 8 GB, still none) | - | - | - | - | 3 stalled run(s) left out | 2.3 | - |

Rows whose min-max ranges overlap are ties and get no rank.

#### Plaintext (pipeline depth 16), 1024 connections

| # | Contender | req/s median | min | max | latency avg (ms) | latency p99 (ms) | errors | steal % max | tied with |
|---:|---|---:|---:|---:|---:|---:|---|---:|---|
| **1** | **Wisp (defaults)** | **623,348** | **506,953** | **663,522** | **8.28** | **n/a** | **-** | **5.7** | **-** |
| **-** | **Wisp, WISP_MAX_CONNS=0 (supplementary)** | **520,331** | **480,904** | **649,788** | **9.73** | **n/a** | **-** | **5.2** | **-** |
| tie | Axum (TFB source) | 381,324 | 323,092 | 391,336 | 21.56 | n/a | - | 6.5 | Actix Web |
| tie | Actix Web (TFB source) | 366,626 | 365,153 | 450,698 | 25.83 | n/a | - | 7.1 | Axum |
| 4 | Fastify (TFB source) | 62,345 | 45,567 | 64,183 | 263 | 4960 | 271 socket | 4.2 | - |
| tie | Express (TFB source) | 36,184 | 32,589 | 39,569 | 295 | 5010 | 246 socket | 6.5 | Hono on Node |
| tie | Hono on Node (TFB source) | 29,602 | 25,357 | 34,082 | 259 | 4420 | 192 socket | 2.9 | Express |
| 7 | Hono on Bun (not TFB source) | 10,375 | 10,195 | 10,421 | 854 | 1670 | - | 1.9 | - |
| 8 | SvelteKit (not TFB) | 8,464 | 7,749 | 9,116 | 514 | 3240 | 83 socket | 6.5 | - |
| - | Next.js (not TFB) | No valid result: no pipelined response completed (heap raised to 8 GB, still none) | - | - | - | - | 3 stalled run(s) left out | 0.2 | - |

Rows whose min-max ranges overlap are ties and get no rank.

#### Plaintext (pipeline depth 16), 4096 connections

| # | Contender | req/s median | min | max | latency avg (ms) | latency p99 (ms) | errors | steal % max | tied with |
|---:|---|---:|---:|---:|---:|---:|---|---:|---|
| **-** | **Wisp, WISP_MAX_CONNS=0 (supplementary)** | **539,506** | **539,174** | **586,425** | **31.49** | **n/a** | **-** | **2.8** | **-** |
| **1** | **Wisp (defaults)** | **532,959** | **460,222** | **546,912** | **37.73** | **n/a** | **-** | **4.4** | **-** |
| 2 | Axum (TFB source) | 446,398 | 417,470 | 454,096 | 67.51 | n/a | - | 2.2 | - |
| 3 | Actix Web (TFB source) | 351,844 | 318,901 | 356,337 | 92.95 | n/a | - | 9.8 | - |
| 4 | Fastify (TFB source) | 58,956 | 49,591 | 61,460 | 246 | 2770 | 152 socket | 3.5 | - |
| tie | Hono on Node (TFB source) | 34,173 | 18,886 | 37,243 | 558 | 5850 | 235 socket | 3.9 | Express |
| tie | Express (TFB source) | 27,554 | 26,800 | 27,714 | 478 | 5710 | 208 socket | 9.9 | Hono on Node |
| 7 | Hono on Bun (not TFB source) | 9,652 | 9,619 | 10,152 | 3400 | 6900 | 1 socket | 4.0 | - |
| 8 | SvelteKit (not TFB) | 8,359 | 8,285 | 9,053 | 547 | 5710 | 149 socket | 1.6 | - |
| - | Next.js (not TFB) | No valid result: no pipelined response completed (heap raised to 8 GB, still none) | - | - | - | - | 1 stalled run(s) left out | 0.6 | - |

Rows whose min-max ranges overlap are ties and get no rank.

#### Plaintext (pipeline depth 16), 16384 connections

| # | Contender | req/s median | min | max | latency avg (ms) | latency p99 (ms) | errors | steal % max | tied with |
|---:|---|---:|---:|---:|---:|---:|---|---:|---|
| **-** | **Wisp, WISP_MAX_CONNS=0 (supplementary)** | **566,387** | **411,488** | **615,279** | **77.72** | **n/a** | **-** | **0.7** | **-** |
| 1 | Axum (TFB source) | 171,231 | 27,410 | 223,946 | 340 | 538 | - | 2.8 | - |
| 2 | Fastify (TFB source) | 17,487 | 16,573 | 18,400 | 2095 | 4110 | 1 stalled run(s) left out | 2.0 | - |
| - | Hono on Node (TFB source) | 16,692 | 16,692 | 16,692 | 3370 | 7720 | one valid run, not ranked, 2 stalled run(s) left out, 830 socket | 0.9 | - |
| 3 | Hono on Bun (not TFB source) | 6,910 | 6,440 | 7,323 | 3720 | 7890 | 2410 socket | 0.7 | - |
| - | SvelteKit (not TFB) | 6,777 | 6,777 | 6,777 | 3840 | 7890 | one valid run, not ranked, 2 stalled run(s) left out, 287 socket | 0.5 | - |
| **-** | **Wisp (defaults)** | **Failed (every run stalled)** | **-** | **-** | **-** | **-** | **73227 non-2xx, 3 stalled run(s) left out** | **8.8** | **-** |
| - | Actix Web (TFB source) | Failed (every run stalled) | - | - | - | - | 3 stalled run(s) left out | 7.6 | - |
| - | Express (TFB source) | Failed (every run stalled) | - | - | - | - | 3 stalled run(s) left out | 3.7 | - |

Rows whose min-max ranges overlap are ties and get no rank.

#### JSON serialization, 16 connections

| # | Contender | req/s median | min | max | latency avg (ms) | latency p99 (ms) | errors | steal % max | tied with |
|---:|---|---:|---:|---:|---:|---:|---|---:|---|
| **-** | **Wisp, WISP_MAX_CONNS=0 (supplementary)** | **110,676** | **108,926** | **113,161** | **0.13** | **0.47** | **-** | **1.2** | **-** |
| tie | Actix Web (TFB source) | 90,199 | 89,072 | 94,100 | 0.32 | 2.68 | - | 2.5 | Axum |
| tie | Axum (TFB source) | 79,105 | 65,609 | 94,820 | 0.29 | 3.11 | - | 12.0 | Actix Web, Wisp |
| **tie** | **Wisp (defaults)** | **71,879** | **65,907** | **78,071** | **0.60** | **6.95** | **-** | **16.2** | **Axum** |
| 4 | Hono on Bun (not TFB source) | 52,353 | 51,695 | 53,469 | 0.37 | 2.29 | - | 1.8 | - |
| tie | Fastify (TFB source) | 21,996 | 17,145 | 23,100 | 0.92 | 6.28 | - | 18.7 | Hono on Node |
| tie | Hono on Node (TFB source) | 17,069 | 16,890 | 17,461 | 2.29 | 35.93 | - | 3.2 | Express, Fastify |
| tie | Express (TFB source) | 16,945 | 15,576 | 17,105 | 1.26 | 8.57 | - | 13.8 | Hono on Node |
| 8 | SvelteKit (not TFB) | 7,990 | 7,662 | 8,274 | 4.15 | 53.90 | - | 5.8 | - |
| 9 | Next.js (not TFB) | 1,499 | 1,174 | 1,665 | 14.27 | 114 | - | 4.7 | - |

Rows whose min-max ranges overlap are ties and get no rank.

#### JSON serialization, 32 connections

| # | Contender | req/s median | min | max | latency avg (ms) | latency p99 (ms) | errors | steal % max | tied with |
|---:|---|---:|---:|---:|---:|---:|---|---:|---|
| **-** | **Wisp, WISP_MAX_CONNS=0 (supplementary)** | **111,845** | **103,039** | **114,315** | **0.23** | **1.45** | **-** | **1.9** | **-** |
| tie | Actix Web (TFB source) | 99,098 | 92,900 | 99,180 | 0.65 | 4.20 | - | 2.5 | Wisp |
| tie | Axum (TFB source) | 86,901 | 77,724 | 90,891 | 0.43 | 3.18 | - | 6.7 | Wisp |
| **tie** | **Wisp (defaults)** | **86,813** | **69,581** | **96,944** | **0.37** | **3.67** | **-** | **14.8** | **Actix Web, Axum** |
| 4 | Hono on Bun (not TFB source) | 54,056 | 49,884 | 62,653 | 0.66 | 3.31 | - | 3.3 | - |
| 5 | Fastify (TFB source) | 21,210 | 19,871 | 22,512 | 1.70 | 9.12 | - | 12.7 | - |
| tie | Express (TFB source) | 16,586 | 15,719 | 16,709 | 2.14 | 10.35 | - | 13.6 | Hono on Node |
| tie | Hono on Node (TFB source) | 15,571 | 9,505 | 15,792 | 3.72 | 46.52 | - | 11.7 | Express, SvelteKit |
| tie | SvelteKit (not TFB) | 9,774 | 8,552 | 11,060 | 8.77 | 201 | - | 2.7 | Hono on Node |
| 9 | Next.js (not TFB) | 1,285 | 1,211 | 1,293 | 30.42 | 192 | - | 5.0 | - |

Rows whose min-max ranges overlap are ties and get no rank.

#### JSON serialization, 64 connections

| # | Contender | req/s median | min | max | latency avg (ms) | latency p99 (ms) | errors | steal % max | tied with |
|---:|---|---:|---:|---:|---:|---:|---|---:|---|
| **-** | **Wisp, WISP_MAX_CONNS=0 (supplementary)** | **111,903** | **101,064** | **117,058** | **0.38** | **1.45** | **-** | **1.6** | **-** |
| **tie** | **Wisp (defaults)** | **96,089** | **85,621** | **103,764** | **0.55** | **2.63** | **-** | **7.1** | **Actix Web, Axum** |
| tie | Actix Web (TFB source) | 89,648 | 85,400 | 101,441 | 0.89 | 5.03 | - | 3.0 | Axum, Wisp |
| tie | Axum (TFB source) | 78,427 | 77,298 | 94,930 | 0.91 | 6.82 | - | 6.7 | Actix Web, Wisp |
| 4 | Hono on Bun (not TFB source) | 59,619 | 54,929 | 64,113 | 1.14 | 4.24 | - | 2.0 | - |
| 5 | Fastify (TFB source) | 21,965 | 20,468 | 22,283 | 3.06 | 10.64 | - | 10.4 | - |
| 6 | Express (TFB source) | 14,746 | 14,509 | 15,351 | 4.65 | 18.06 | - | 15.7 | - |
| tie | SvelteKit (not TFB) | 10,378 | 8,989 | 10,757 | 6.37 | 17.41 | - | 1.5 | Hono on Node |
| tie | Hono on Node (TFB source) | 8,988 | 8,708 | 10,864 | 9.48 | 73.17 | - | 21.0 | SvelteKit |
| 9 | Next.js (not TFB) | 1,524 | 1,498 | 1,698 | 49.06 | 298 | - | 2.3 | - |

Rows whose min-max ranges overlap are ties and get no rank.

#### JSON serialization, 128 connections

| # | Contender | req/s median | min | max | latency avg (ms) | latency p99 (ms) | errors | steal % max | tied with |
|---:|---|---:|---:|---:|---:|---:|---|---:|---|
| **-** | **Wisp, WISP_MAX_CONNS=0 (supplementary)** | **109,058** | **106,815** | **111,908** | **0.70** | **2.33** | **-** | **1.1** | **-** |
| **tie** | **Wisp (defaults)** | **107,834** | **107,405** | **114,306** | **0.69** | **2.06** | **-** | **1.2** | **Actix Web** |
| tie | Actix Web (TFB source) | 106,246 | 101,186 | 114,139 | 1.11 | 5.23 | - | 1.1 | Wisp |
| 3 | Axum (TFB source) | 92,662 | 70,648 | 96,863 | 1.30 | 4.63 | - | 5.7 | - |
| 4 | Hono on Bun (not TFB source) | 51,580 | 50,493 | 52,293 | 2.54 | 7.78 | - | 2.6 | - |
| 5 | Fastify (TFB source) | 19,977 | 17,809 | 20,936 | 6.75 | 22.77 | - | 12.1 | - |
| 6 | Express (TFB source) | 13,795 | 11,936 | 14,442 | 10.06 | 32.47 | - | 16.5 | - |
| 7 | Hono on Node (TFB source) | 9,354 | 8,902 | 11,671 | 17.39 | 115 | - | 16.7 | - |
| 8 | SvelteKit (not TFB) | 7,714 | 6,960 | 8,883 | 18.28 | 151 | - | 3.8 | - |
| 9 | Next.js (not TFB) | 1,513 | 1,132 | 1,669 | 117 | 1460 | - | 5.3 | - |

Rows whose min-max ranges overlap are ties and get no rank.

#### JSON serialization, 256 connections

| # | Contender | req/s median | min | max | latency avg (ms) | latency p99 (ms) | errors | steal % max | tied with |
|---:|---|---:|---:|---:|---:|---:|---|---:|---|
| **-** | **Wisp, WISP_MAX_CONNS=0 (supplementary)** | **111,299** | **107,977** | **120,500** | **1.25** | **3.13** | **-** | **0.8** | **-** |
| 1 | Actix Web (TFB source) | 108,428 | 105,161 | 109,280 | 1.77 | 5.92 | - | 0.8 | - |
| **2** | **Wisp (defaults)** | **91,876** | **86,957** | **99,850** | **1.75** | **7.18** | **-** | **3.2** | **-** |
| 3 | Axum (TFB source) | 60,465 | 57,541 | 81,434 | 4.12 | 14.13 | - | 6.0 | - |
| 4 | Hono on Bun (not TFB source) | 47,379 | 39,410 | 48,390 | 5.50 | 16.00 | - | 4.5 | - |
| tie | Fastify (TFB source) | 21,249 | 14,340 | 21,494 | 17.20 | 225 | - | 13.0 | Express |
| tie | Express (TFB source) | 18,023 | 15,567 | 18,298 | 18.06 | 161 | - | 7.7 | Fastify |
| 7 | Hono on Node (TFB source) | 13,084 | 12,376 | 13,572 | 31.18 | 543 | - | 6.1 | - |
| 8 | SvelteKit (not TFB) | 6,062 | 5,580 | 8,512 | 81.65 | 1410 | - | 5.2 | - |
| 9 | Next.js (not TFB) | 1,370 | 1,092 | 1,533 | 428 | 5490 | 139 socket | 7.2 | - |

Rows whose min-max ranges overlap are ties and get no rank.

#### JSON serialization, 512 connections

| # | Contender | req/s median | min | max | latency avg (ms) | latency p99 (ms) | errors | steal % max | tied with |
|---:|---|---:|---:|---:|---:|---:|---|---:|---|
| **-** | **Wisp, WISP_MAX_CONNS=0 (supplementary)** | **100,818** | **93,145** | **105,347** | **2.73** | **7.59** | **-** | **1.0** | **-** |
| **tie** | **Wisp (defaults)** | **73,714** | **65,578** | **73,946** | **3.89** | **11.79** | **-** | **6.1** | **Actix Web** |
| tie | Actix Web (TFB source) | 71,315 | 68,764 | 101,055 | 5.92 | 16.53 | - | 2.7 | Wisp |
| tie | Axum (TFB source) | 47,070 | 41,755 | 64,023 | 10.59 | 24.08 | - | 7.0 | Hono on Bun |
| tie | Hono on Bun (not TFB source) | 28,439 | 26,911 | 44,598 | 17.93 | 47.98 | - | 8.9 | Axum |
| 5 | Fastify (TFB source) | 24,163 | 23,503 | 24,449 | 68.22 | 1600 | - | 2.8 | - |
| 6 | Express (TFB source) | 22,644 | 19,491 | 22,858 | 59.97 | 1370 | - | 2.7 | - |
| 7 | Hono on Node (TFB source) | 15,349 | 14,608 | 17,748 | 127 | 2520 | - | 3.2 | - |
| 8 | SvelteKit (not TFB) | 6,698 | 5,707 | 7,340 | 387 | 5670 | 124 socket | 4.9 | - |
| 9 | Next.js (not TFB) | 1,715 | 1,699 | 1,854 | 308 | 4830 | 240 socket | 1.0 | - |

Rows whose min-max ranges overlap are ties and get no rank.

## Noise: the same binary at different moments

`./run.sh noise` alternates Wisp (defaults) and the supplementary build (identical code, only
`WISP_MAX_CONNS` differs, which changes nothing at 16 or 256 connections) for 3 rounds, JSON only:

| Binary | Level | round 1 | round 2 | round 3 | steal % of CPU (r1/r2/r3) |
|---|---:|---:|---:|---:|---|
| Wisp (defaults) | 16 | 112,192 | 110,419 | 87,941 | 1.11 / 1.08 / 5.24 |
| Wisp (defaults) | 256 | 103,688 | 86,482 | 84,953 | 1.18 / 2.69 / 4.1 |
| Wisp, WISP_MAX_CONNS=0 (supplementary) | 16 | 114,386 | 93,085 | 111,173 | 0.76 / 3.59 / 1.04 |
| Wisp, WISP_MAX_CONNS=0 (supplementary) | 256 | 104,835 | 93,644 | 95,005 | 1.05 / 1.61 / 1.81 |

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
- **Pipelined rows have no p99** (wrk), and latency is queueing under a 16-deep pipeline, not
  request latency.
- Next.js has no valid pipelined plaintext result: no 16-deep pipelined response completed within wrk's timeout. It was restarted
  before the JSON test (`WORKLOADS=json ./run.sh bench next`); its plaintext numbers are what was
  measured before and during that failure (the 2 GB V8 heap was first suspected). Re-run with `NODE_OPTIONS=--max-old-space-size=8192` (run.sh `NEXT_HEAP_MB`, 7.9 GB host, two
  runs, raw in `raw/next-heap8192/`): still 0 req/s at almost every level (best single run 1,489 at 4096), no
  out-of-memory message, so the heap was not the cause; Next.js is CPU-saturated and wrk completes no
  16-deep pipelined response within its timeout. Steal peaked at 12 and 16 % (mean 2.5 and 3.5 %) in 5 s samples, so
  treat those runs as indicative only. SvelteKit and Next.js are plain route handlers, not
  optimised entries, and not TFB code.
- Single pass per contender, in the order Wisp, Axum, Actix Web, Express, Fastify, Hono (Node), Hono (Bun), SvelteKit, Next.js, then the supplementary build: contenders measured later
  saw a different moment of the host than Wisp did (the noise experiment shows the size of that).
- Wisp was not tuned; contenders were not tuned beyond what their TFB entries or docs recommend.

## Omitted

- none

Nothing else was left out. Hono on Bun, SvelteKit and Next.js are labelled as non-TFB above.

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
