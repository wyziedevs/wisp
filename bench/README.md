# Benchmarks

Wisp against ASP.NET Core on the same machine, doing the same work.

```
powershell -ExecutionPolicy Bypass -File bench\run.ps1 [-Connections 64] [-Duration 10] [-Warmup 5]
```

- `app/`: the Wisp side. `/fortunes` is TechEmpower's fortunes test without
  the database: copy 12 rows, add one, sort by message, render an HTML table
  with escaping. `/plaintext` returns `Hello, World!`.
- `aspnet/`: the same two endpoints, written the way the ASP.NET Core docs
  and templates do: `/fortunes` as a Razor Page, `/fortunes-blazor` as a Blazor
  component (static SSR), and `/plaintext` as a minimal API. Logging is set to
  `Warning` as the templates' appsettings do, and the HTML encoder emits
  non-ASCII as is, like Wisp.
- `load/`: the load generator. It is closed-loop: each connection sends a
  request and waits for the whole response before the next (no pipelining),
  like wrk and bombardier by default. It records latency in a log-linear
  histogram and counts only requests that complete in the measured window.

`run.ps1` builds both in release mode and runs each server alone, pinned to
the first half of the logical CPUs, with the load generator on the other
half, so the two never compete for a core. Besides throughput and latency it
reports the server's CPU time per request (throughput alone can be capped by
the load generator or the OS network stack), peak memory, and the time from
launch to the first response.

## Results

2026-09-28. Windows 10, AMD Ryzen 7 7800X3D (8 cores, 16 threads). Servers
on logical CPUs 0-7 (four cores with SMT), load on 8-15. Rust 1.97, .NET
10.0.302. 64 connections, 5 s warmup, 10 s measured.

| Server       | Path             |   req/s | p50     | p99     | p99.9   | CPU µs/req | Peak MB | First response |
|--------------|------------------|--------:|---------|---------|---------|-----------:|--------:|---------------:|
| Wisp         | /plaintext       | 753,852 | 0.07 ms | 0.37 ms | 2.69 ms |        8.0 |       6 |          18 ms |
| Wisp         | /fortunes        | 705,459 | 0.07 ms | 0.37 ms | 2.05 ms |        9.2 |       6 |          18 ms |
| ASP.NET Core | /plaintext       | 652,385 | 0.09 ms | 0.33 ms | 0.72 ms |       10.4 |      77 |         227 ms |
| ASP.NET Core | /fortunes        | 320,059 | 0.19 ms | 0.45 ms | 0.61 ms |       23.6 |     102 |         227 ms |
| ASP.NET Core | /fortunes-blazor | 167,005 | 0.35 ms | 0.67 ms | 3.39 ms |       46.7 |     154 |         227 ms |

On fortunes, the test that renders a page, Wisp serves 2.2× the requests of
Razor Pages and 4.2× those of Blazor, at 39% and 20% of the CPU per request,
in 6 MB instead of 100-150 MB.

**Tail latency under saturation.** At 64 connections every server CPU is
busy, and Wisp's p99.9 is worse than Razor Pages'. Wisp runs one runtime per
core with no work stealing, so when the OS preempts a worker (this is a
desktop with other processes), that worker's connections wait for it.
Kestrel's shared thread pool lets another thread pick them up. Below
saturation Wisp's tail is the better one:

| Server       | Path      | Connections |   req/s | p99     | p99.9   | max     |
|--------------|-----------|------------:|--------:|---------|---------|---------|
| Wisp         | /fortunes |           8 | 409,440 | 0.03 ms | 0.07 ms | 0.43 ms |
| Wisp         | /fortunes |          16 | 538,005 | 0.06 ms | 0.13 ms | 0.83 ms |
| ASP.NET Core | /fortunes |           8 | 211,359 | 0.06 ms | 0.26 ms | 1.55 ms |
| ASP.NET Core | /fortunes |          16 | 298,328 | 0.12 ms | 0.31 ms | 1.15 ms |

**Why thread per core.** With one multi-threaded tokio runtime, plaintext
topped out at 418k req/s with 3.4 of 8 cores busy: every socket event goes
through one I/O driver. With a runtime per core the same code scales
linearly (190k, 422k, 715k req/s at 1, 2, 4 threads) until the load
generator becomes the limit.

## Caveats

- Loopback on one machine, and on Windows, where tokio's socket readiness is
  emulated on top of IOCP. Production servers are usually Linux (epoll).
- The load generator runs on four cores and is likely the limit for Wisp at
  8 threads (the servers were not at 100% CPU).
- No database. Fortunes measures routing, rendering, escaping and HTTP.
- Kestrel sends `/plaintext` chunked (the minimal API default for a returned
  string), so its responses are 35 bytes larger.
