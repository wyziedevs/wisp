### workerd

c=64, 10 s runs, median of 5, cold start median of 15; 2026-10-06, CPU steal mean 0% (max 0%), ip_local_port_range 1024 65535. Each cell: req/s (place among the frameworks; a second Wisp variant is not counted against the first).

| framework | `/` | `/list1000` | `/json-big` | `/params` | cold start ms | RSS MB after load |
|---|---|---|---|---|---|---|
| hono | Failed | Failed | Failed | Failed | 63 (#2) | 309 (#2) |
| **wisp** | Failed | Failed | **1,996 (#1)** | Failed | **93 (#3)** | **407 (#3)** |
| itty | 3,966 (#1) | Failed | Failed | Failed | 60 (#1) | 517 (#5) |
| sveltekit | Failed | 366 (#1) | Failed | 2,298 (#1) | 97 (#4) | 799 (#7) |
| astro | Failed | Failed | 1,152 (#2) | Failed | 147 (#6) | 469 (#4) |
| react-router | 1,959 (#2) | 193 (#2) | Failed | Failed | 120 (#5) | 301 (#1) |
| next | Failed | 44 (#3) | Failed | 338 (#2) | 489 (#7) | 561 (#6) |

### Node

c=64, 10 s runs, median of 5, cold start median of 15; 2026-10-05, CPU steal mean 0.01% (max 3%), ip_local_port_range 1024 65535. Each cell: req/s (place among the frameworks; a second Wisp variant is not counted against the first).

| framework | `/` | `/list1000` | `/json-big` | `/params` | cold start ms | RSS MB after load |
|---|---|---|---|---|---|---|
| **wisp raw** | **35,201 (#1)** | **3,354 (#1)** | **6,817 (#1)** | **30,911 (#1)** | **158 (#2)** | **83 (#1)** |
| **wisp node:http** | **12,475 (#3)** | **2,823 (#1)** | **5,183 (#2)** | **12,186 (#2)** | **148 (#1)** | **103 (#4)** |
| fastify | 17,319 (#2) | 954 (#3) | 5,569 (#2) | 15,042 (#2) | 268 (#5) | 93 (#2) |
| hono | 14,149 (#3) | 1,013 (#2) | 4,660 (#3) | 9,616 (#3) | 154 (#2) | 97 (#4) |
| express | 7,405 (#4) | 836 (#4) | 3,191 (#4) | 7,144 (#4) | 207 (#4) | 94 (#3) |
| sveltekit | 3,801 (#5) | 439 (#5) | 2,699 (#5) | 4,153 (#5) | 181 (#3) | 185 (#5) |
| next | 849 (#6) | 45 (#6) | 883 (#6) | 1,050 (#6) | 645 (#6) | 445 (#6) |

### Bun

c=64, 10 s runs, median of 5, cold start median of 15; 2026-10-05, CPU steal mean 0% (max 2%), ip_local_port_range 1024 65535. Each cell: req/s (place among the frameworks; a second Wisp variant is not counted against the first).

| framework | `/` | `/list1000` | `/json-big` | `/params` | cold start ms | RSS MB after load |
|---|---|---|---|---|---|---|
| **wisp raw** | **47,282 (#1)** | **4,037 (#1)** | **9,315 (#1)** | **41,357 (#1)** | **71 (#2)** | **39 (#2)** |
| **wisp Bun.serve** | **26,405 (#3)** | **3,660 (#1)** | **7,613 (#1)** | **17,775 (#3)** | **63 (#2)** | **41 (#2)** |
| elysia | 43,164 (#2) | 872 (#2) | 6,962 (#2) | 18,489 (#3) | 146 (#3) | 49 (#3) |
| hono | 37,001 (#3) | 840 (#3) | 6,045 (#3) | 24,036 (#2) | 60 (#1) | 38 (#1) |

### Deno

c=64, 10 s runs, median of 5, cold start median of 15; 2026-10-05, CPU steal mean 0% (max 2%), ip_local_port_range 1024 65535. Each cell: req/s (place among the frameworks; a second Wisp variant is not counted against the first).

| framework | `/` | `/list1000` | `/json-big` | `/params` | cold start ms | RSS MB after load |
|---|---|---|---|---|---|---|
| **wisp raw** | **40,877 (#2)** | **3,021 (#1)** | **5,485 (#2)** | **37,094 (#1)** | **113 (#3)** | **98 (#3)** |
| hono | 43,146 (#1) | 1,015 (#2) | 7,212 (#1) | 29,262 (#2) | 65 (#1) | 53 (#1) |
| **wisp Deno.serve** | **31,995 (#2)** | **2,726 (#1)** | **5,017 (#4)** | **20,670 (#2)** | **108 (#2)** | **63 (#2)** |
| oak | 15,117 (#4) | 827 (#3) | 5,229 (#3) | 12,748 (#3) | 174 (#4) | 86 (#3) |
| fresh | 21,626 (#3) | 682 (#4) | 5,188 (#4) | 8,517 (#4) | 111 (#3) | 100 (#4) |

### Where Wisp is below 3rd or behind Hono

Gap is how far behind in %: lower req/s, or higher cold start and memory. Gap to 3rd is against the framework in 3rd place (Wisp not counted).

| host | Wisp variant | metric | place | Wisp | Hono | behind Hono | behind 3rd |
|---|---|---|---|---|---|---|---|
| workerd | wisp | cold start ms | #3 | 93 | 63 | 48% | - |
| workerd | wisp | RSS MB after load | #3 | 407 | 309 | 32% | - |
| Node | wisp raw | cold start ms | #2 | 158 | 154 | 3% | - |
| Node | wisp node:http | `/` | #3 | 12,475 | 14,149 | 12% | - |
| Node | wisp node:http | RSS MB after load | #4 | 103 | 97 | 6% | 6% (hono) |
| Bun | wisp raw | cold start ms | #2 | 71 | 60 | 18% | - |
| Bun | wisp raw | RSS MB after load | #2 | 39 | 38 | 3% | - |
| Bun | wisp Bun.serve | `/` | #3 | 26,405 | 37,001 | 29% | - |
| Bun | wisp Bun.serve | `/params` | #3 | 17,775 | 24,036 | 26% | - |
| Bun | wisp Bun.serve | cold start ms | #2 | 63 | 60 | 5% | - |
| Bun | wisp Bun.serve | RSS MB after load | #2 | 41 | 38 | 8% | - |
| Deno | wisp raw | `/` | #2 | 40,877 | 43,146 | 5% | - |
| Deno | wisp raw | `/json-big` | #2 | 5,485 | 7,212 | 24% | - |
| Deno | wisp raw | cold start ms | #3 | 113 | 65 | 73% | - |
| Deno | wisp raw | RSS MB after load | #3 | 98 | 53 | 85% | - |
| Deno | wisp Deno.serve | `/` | #2 | 31,995 | 43,146 | 26% | - |
| Deno | wisp Deno.serve | `/json-big` | #4 | 5,017 | 7,212 | 30% | 3% (fresh) |
| Deno | wisp Deno.serve | `/params` | #2 | 20,670 | 29,262 | 29% | - |
| Deno | wisp Deno.serve | cold start ms | #2 | 108 | 65 | 66% | - |
| Deno | wisp Deno.serve | RSS MB after load | #2 | 63 | 53 | 19% | - |
