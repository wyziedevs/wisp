### workerd

c=64, 10 s runs, median of 5, cold start median of 15; 2026-10-04. Each cell: req/s (place among the frameworks; a second Wisp variant is not counted against the first).

| framework | `/` | `/list1000` | `/json-big` | `/params` | cold start ms | RSS MB after load |
|---|---|---|---|---|---|---|
| itty | 3,564 (#3) | 337 (#2) | 1,570 (#1) | 4,163 (#1) | 53 (#1) | 426 (#4) |
| hono | 4,217 (#1) | 330 (#3) | 1,537 (#2) | 4,076 (#2) | 58 (#2) | 357 (#2) |
| **wisp** | **4,002 (#2)** | **626 (#1)** | **1,395 (#3)** | **3,581 (#3)** | **98 (#5)** | **309 (#1)** |
| sveltekit | 1,974 (#4) | 148 (#5) | 1,128 (#4) | 2,098 (#5) | 80 (#3) | 838 (#7) |
| astro | 1,768 (#5) | 275 (#4) | 918 (#6) | 2,159 (#4) | 112 (#6) | 585 (#5) |
| react-router | 1,757 (#6) | 76 (#6) | 1,056 (#5) | 1,373 (#6) | 93 (#4) | 387 (#3) |
| next | 286 (#7) | 19 (#7) | 204 (#7) | 290 (#7) | 450 (#7) | 601 (#6) |

### Node

c=64, 10 s runs, median of 5, cold start median of 15; 2026-10-04. Each cell: req/s (place among the frameworks; a second Wisp variant is not counted against the first).

| framework | `/` | `/list1000` | `/json-big` | `/params` | cold start ms | RSS MB after load |
|---|---|---|---|---|---|---|
| **wisp raw** | **33,575 (#1)** | **2,972 (#1)** | **7,767 (#1)** | **20,022 (#1)** | **203 (#2)** | **120 (#3)** |
| **wisp node:http** | **11,317 (#3)** | **2,556 (#1)** | **5,424 (#1)** | **8,285 (#2)** | **156 (#1)** | **89 (#1)** |
| hono | 12,213 (#2) | 847 (#3) | 4,457 (#2) | 7,320 (#3) | 187 (#2) | 95 (#3) |
| fastify | 11,882 (#3) | 785 (#4) | 4,256 (#3) | 8,994 (#2) | 318 (#5) | 94 (#2) |
| express | 6,755 (#4) | 860 (#2) | 3,421 (#4) | 5,889 (#4) | 251 (#4) | 157 (#4) |
| sveltekit | 4,431 (#5) | 447 (#5) | 2,493 (#5) | 3,072 (#5) | 206 (#3) | 279 (#5) |
| next | 962 (#6) | 50 (#6) | 922 (#6) | 720 (#6) | 835 (#6) | 496 (#6) |

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
| workerd | wisp | `/` | #2 | 4,002 | 4,217 | 5% | - |
| workerd | wisp | `/json-big` | #3 | 1,395 | 1,537 | 9% | - |
| workerd | wisp | `/params` | #3 | 3,581 | 4,076 | 12% | - |
| workerd | wisp | cold start ms | #5 | 98 | 58 | 69% | 22% (sveltekit) |
| Node | wisp raw | cold start ms | #2 | 203 | 187 | 8% | - |
| Node | wisp raw | RSS MB after load | #3 | 120 | 95 | 26% | - |
| Node | wisp node:http | `/` | #3 | 11,317 | 12,213 | 7% | - |
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
