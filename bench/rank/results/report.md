### workerd

c=64, 10 s runs, median of 5, cold start median of 15; 2026-10-05. Each cell: req/s (place among the frameworks; a second Wisp variant is not counted against the first).

| framework | `/` | `/list1000` | `/json-big` | `/params` | cold start ms | RSS MB after load |
|---|---|---|---|---|---|---|
| hono | 1,075 (#1) | 238 (#3) | 677 (#2) | 1,142 (#1) | 206 (#2) | 284 (#3) |
| **wisp** | **1,069 (#2)** | **265 (#1)** | **672 (#3)** | **975 (#2)** | **305 (#4)** | **250 (#1)** |
| itty | 1,035 (#3) | 260 (#2) | 747 (#1) | 957 (#3) | 186 (#1) | 427 (#5) |
| sveltekit | 600 (#5) | 104 (#5) | 518 (#4) | 667 (#4) | 286 (#3) | 662 (#7) |
| astro | 610 (#4) | 181 (#4) | 508 (#5) | 657 (#5) | 428 (#6) | 442 (#6) |
| react-router | 580 (#6) | 45 (#6) | 374 (#6) | 520 (#6) | 384 (#5) | 281 (#2) |
| next | 111 (#7) | 13 (#7) | 77 (#7) | 94 (#7) | 1653 (#7) | 422 (#4) |

### Node

c=64, 10 s runs, median of 5, cold start median of 15; 2026-10-05. Each cell: req/s (place among the frameworks; a second Wisp variant is not counted against the first).

| framework | `/` | `/list1000` | `/json-big` | `/params` | cold start ms | RSS MB after load |
|---|---|---|---|---|---|---|
| **wisp raw** | **3,763 (#1)** | **643 (#1)** | **981 (#1)** | **4,244 (#1)** | **1107 (#1)** | **89 (#1)** |
| **wisp node:http** | **2,155 (#1)** | **574 (#1)** | **762 (#3)** | **1,992 (#2)** | **1190 (#1)** | **95 (#4)** |
| hono | 1,336 (#3) | 158 (#3) | 892 (#2) | 1,904 (#3) | 1219 (#2) | 94 (#3) |
| fastify | 1,902 (#2) | 156 (#4) | 790 (#3) | 2,281 (#2) | 2188 (#5) | 93 (#2) |
| express | 778 (#4) | 162 (#2) | 615 (#4) | 1,264 (#4) | 1514 (#4) | 94 (#3) |
| sveltekit | 409 (#5) | 73 (#5) | 385 (#5) | 715 (#5) | 1411 (#3) | 147 (#5) |
| next | 125 (#6) | 9 (#6) | 85 (#6) | 71 (#6) | 4295 (#6) | 252 (#6) |

### Bun

c=64, 10 s runs, median of 5, cold start median of 15; 2026-10-05. Each cell: req/s (place among the frameworks; a second Wisp variant is not counted against the first).

| framework | `/` | `/list1000` | `/json-big` | `/params` | cold start ms | RSS MB after load |
|---|---|---|---|---|---|---|
| **wisp raw** | **5,906 (#1)** | **556 (#1)** | **1,467 (#1)** | **5,463 (#1)** | **321 (#2)** | **40 (#2)** |
| **wisp Bun.serve** | **3,279 (#3)** | **687 (#1)** | **1,192 (#1)** | **3,010 (#2)** | **281 (#2)** | **41 (#2)** |
| hono | 4,937 (#3) | 138 (#2) | 1,010 (#2) | 3,372 (#2) | 201 (#1) | 39 (#1) |
| elysia | 5,374 (#2) | 119 (#3) | 1,009 (#3) | 2,819 (#3) | 668 (#3) | 51 (#3) |

### Deno

c=64, 10 s runs, median of 5, cold start median of 15; 2026-10-05. Each cell: req/s (place among the frameworks; a second Wisp variant is not counted against the first).

| framework | `/` | `/list1000` | `/json-big` | `/params` | cold start ms | RSS MB after load |
|---|---|---|---|---|---|---|
| **wisp raw** | **4,881 (#2)** | **666 (#1)** | **1,315 (#2)** | **4,468 (#1)** | **630 (#3)** | **68 (#2)** |
| hono | 5,891 (#1) | 231 (#2) | 1,427 (#1) | 4,137 (#2) | 376 (#1) | 52 (#1) |
| **wisp Deno.serve** | **4,360 (#2)** | **553 (#1)** | **1,140 (#2)** | **3,203 (#2)** | **508 (#2)** | **67 (#2)** |
| oak | 2,321 (#4) | 219 (#3) | 957 (#4) | 1,943 (#3) | 941 (#4) | 84 (#3) |
| fresh | 3,113 (#3) | 167 (#4) | 1,136 (#3) | 1,413 (#4) | 623 (#3) | 94 (#4) |

### Where Wisp is below 3rd or behind Hono

Gap is how far behind in %: lower req/s, or higher cold start and memory. Gap to 3rd is against the framework in 3rd place (Wisp not counted).

| host | Wisp variant | metric | place | Wisp | Hono | behind Hono | behind 3rd |
|---|---|---|---|---|---|---|---|
| workerd | wisp | `/` | #2 | 1,069 | 1,075 | 1% | - |
| workerd | wisp | `/json-big` | #3 | 672 | 677 | 1% | - |
| workerd | wisp | `/params` | #2 | 975 | 1,142 | 15% | - |
| workerd | wisp | cold start ms | #4 | 305 | 206 | 48% | 7% (sveltekit) |
| Node | wisp node:http | `/json-big` | #3 | 762 | 892 | 15% | - |
| Node | wisp node:http | RSS MB after load | #4 | 95 | 94 | 1% | 1% (express) |
| Bun | wisp raw | cold start ms | #2 | 321 | 201 | 60% | - |
| Bun | wisp raw | RSS MB after load | #2 | 40 | 39 | 3% | - |
| Bun | wisp Bun.serve | `/` | #3 | 3,279 | 4,937 | 34% | - |
| Bun | wisp Bun.serve | `/params` | #2 | 3,010 | 3,372 | 11% | - |
| Bun | wisp Bun.serve | cold start ms | #2 | 281 | 201 | 40% | - |
| Bun | wisp Bun.serve | RSS MB after load | #2 | 41 | 39 | 5% | - |
| Deno | wisp raw | `/` | #2 | 4,881 | 5,891 | 17% | - |
| Deno | wisp raw | `/json-big` | #2 | 1,315 | 1,427 | 8% | - |
| Deno | wisp raw | cold start ms | #3 | 630 | 376 | 67% | - |
| Deno | wisp raw | RSS MB after load | #2 | 68 | 52 | 31% | - |
| Deno | wisp Deno.serve | `/` | #2 | 4,360 | 5,891 | 26% | - |
| Deno | wisp Deno.serve | `/json-big` | #2 | 1,140 | 1,427 | 20% | - |
| Deno | wisp Deno.serve | `/params` | #2 | 3,203 | 4,137 | 23% | - |
| Deno | wisp Deno.serve | cold start ms | #2 | 508 | 376 | 35% | - |
| Deno | wisp Deno.serve | RSS MB after load | #2 | 67 | 52 | 29% | - |
