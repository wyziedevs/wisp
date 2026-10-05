### workerd

c=64, 10 s runs, median of 5, cold start median of 15; 2026-10-04. Each cell: req/s (place among the frameworks; a second Wisp variant is not counted against the first).

| framework | `/` | `/list1000` | `/json-big` | `/params` | cold start ms | RSS MB after load |
|---|---|---|---|---|---|---|
| hono | 6,186 (#1) | 795 (#3) | 3,359 (#1) | 4,774 (#1) | 75 (#2) | 303 (#1) |
| **wisp** | **5,579 (#2)** | **1,902 (#1)** | **3,243 (#2)** | **4,771 (#2)** | **100 (#3)** | **374 (#2)** |
| itty | 4,766 (#3) | 855 (#2) | 3,002 (#3) | 4,130 (#3) | 70 (#1) | 754 (#6) |
| astro | 2,986 (#4) | 698 (#4) | 2,203 (#5) | 2,477 (#4) | 179 (#6) | 477 (#4) |
| sveltekit | 2,884 (#5) | 418 (#5) | 2,296 (#4) | 2,223 (#5) | 120 (#4) | 1581 (#7) |
| react-router | 2,351 (#6) | 226 (#6) | 1,905 (#6) | 1,849 (#6) | 151 (#5) | 425 (#3) |
| next | 457 (#7) | 45 (#7) | 384 (#7) | 399 (#7) | 631 (#7) | 631 (#5) |

### Node

c=64, 10 s runs, median of 5, cold start median of 15; 2026-10-04. Each cell: req/s (place among the frameworks; a second Wisp variant is not counted against the first).

| framework | `/` | `/list1000` | `/json-big` | `/params` | cold start ms | RSS MB after load |
|---|---|---|---|---|---|---|
| **wisp raw** | **33,942 (#1)** | **3,165 (#1)** | **7,959 (#1)** | **30,533 (#1)** | **166 (#1)** | **82 (#1)** |
| **wisp node:http** | **13,151 (#3)** | **3,001 (#1)** | **5,940 (#1)** | **11,627 (#2)** | **152 (#1)** | **290 (#5)** |
| hono | 14,196 (#2) | 1,043 (#3) | 4,583 (#2) | 9,131 (#3) | 167 (#2) | 100 (#3) |
| fastify | 13,661 (#3) | 1,049 (#2) | 4,496 (#3) | 11,862 (#2) | 312 (#5) | 95 (#2) |
| express | 7,325 (#4) | 927 (#4) | 3,484 (#4) | 6,671 (#4) | 231 (#4) | 226 (#4) |
| sveltekit | 4,802 (#5) | 516 (#5) | 2,629 (#5) | 4,300 (#5) | 187 (#3) | 269 (#5) |
| next | 1,211 (#6) | 66 (#6) | 1,040 (#6) | 1,227 (#6) | 725 (#6) | 505 (#6) |

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
| workerd | wisp | `/` | #2 | 5,579 | 6,186 | 10% | - |
| workerd | wisp | `/json-big` | #2 | 3,243 | 3,359 | 3% | - |
| workerd | wisp | `/params` | #2 | 4,771 | 4,774 | 0% | - |
| workerd | wisp | cold start ms | #3 | 100 | 75 | 34% | - |
| workerd | wisp | RSS MB after load | #2 | 374 | 303 | 23% | - |
| Node | wisp node:http | `/` | #3 | 13,151 | 14,196 | 7% | - |
| Node | wisp node:http | RSS MB after load | #5 | 290 | 100 | 190% | 28% (express) |
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
