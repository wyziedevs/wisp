### workerd

c=64, 10 s runs, median of 5, cold start median of 15; 2026-10-06, CPU steal mean 0.42% (max 8%), ip_local_port_range 1024 65535; connection resets tolerated: redo up to 3 (3 tries per run, the last try kept, the same rule for every app); runs redone after connection resets (requests reset): wisp / 1 (run 4: 6,445 then 6,801 req/s), hono / 2 (run 2: 6,328 then 6,172 req/s), itty / 20 (run 1: 7,345 then 6,773 req/s; run 3: 6,062 then 5,648 then 5,887 req/s), astro / 3 (run 2: 3,244 then 2,709 req/s), react-router / 11 (run 1: 2,899 then 2,767 req/s; run 5: 2,672 then 2,616 req/s), wisp /list1000 5 (run 2: 2,083 then 1,800 req/s), hono /list1000 13 (run 1: 865 then 925 req/s; run 3: 1,009 then 943 req/s; run 4: 869 then 1,002 req/s), astro /list1000 5 (run 1: 808 then 791 req/s), wisp /json-big 1 (run 5: 3,266 then 3,173 req/s), sveltekit /json-big 1 (run 5: 2,335 then 2,437 req/s), react-router /json-big 1 (run 1: 1,852 then 1,953 req/s), next /json-big 1 (run 4: 293 then 298 req/s), itty /params/42?q=hello%20world&x=1 5 (run 1: 4,307 then 5,160 req/s; run 2: 5,780 then 5,262 req/s), astro /params/42?q=hello%20world&x=1 6 (run 5: 2,841 then 2,578 req/s), react-router /params/42?q=hello%20world&x=1 2 (run 1: 2,226 then 1,941 req/s); before every run: server under 5% of a core over 2 s before every try, 60 s at most; per-run steal max 1.8%. Each cell: req/s (place among the frameworks; a second Wisp variant is not counted against the first; a cell that is not idle, has no drain, fewer than 3 runs, steal over 10% in a run or failed requests is shown unranked with the reason).

| framework | `/` | `/list1000` | `/json-big` | `/params` | cold start ms | RSS MB after load |
|---|---|---|---|---|---|---|
| **wisp** | **6,801 (#2)** | **2,045 (#1)** | **3,360 (#2)** | **5,824 (#2)** | **66 (#3)** | **472 (#3)** |
| hono | 6,880 (#1) | 943 (#3) | 3,353 (#3) | 6,074 (#1) | 45 (#2) | 389 (#2) |
| itty | 5,887 (#3) | 994 (#2) | 3,474 (#1) | 5,209 (#3) | 43 (#1) | 810 (#6) |
| sveltekit | 3,372 (#5) | 511 (#5) | 2,303 (#4) | 3,043 (#4) | 76 (#4) | 1971 (#7) |
| astro | 3,532 (#4) | 873 (#4) | 2,265 (#5) | 3,031 (#5) | 106 (#6) | 486 (#4) |
| react-router | 2,653 (#6) | 257 (#6) | 1,791 (#6) | 2,330 (#6) | 85 (#5) | 369 (#1) |
| next | 519 (#7) | 51 (#7) | 353 (#7) | 441 (#7) | 350 (#7) | 645 (#5) |

### Node

c=64, 10 s runs, median of 5, cold start median of 15; 2026-10-06, CPU steal mean 0.92% (max 15%), ip_local_port_range 1024 65535; connection resets tolerated: redo up to 3 (3 tries per run, the last try kept, the same rule for every app); before every run: server under 5% of a core over 2 s before every try, 60 s at most; per-run steal max 4.4%. Each cell: req/s (place among the frameworks; a second Wisp variant is not counted against the first; a cell that is not idle, has no drain, fewer than 3 runs, steal over 10% in a run or failed requests is shown unranked with the reason).

| framework | `/` | `/list1000` | `/json-big` | `/params` | cold start ms | RSS MB after load |
|---|---|---|---|---|---|---|
| **wisp raw** | **38,790 (#1)** | **3,458 (#1)** | **8,507 (#1)** | **35,401 (#1)** | **176 (#2)** | **83 (#1)** |
| fastify | 19,451 (#2) | 1,030 (#2) | 6,376 (#2) | 15,854 (#2) | 300 (#5) | 94 (#2) |
| **wisp node:http** | **16,683 (#3)** | **3,027 (#1)** | **5,979 (#3)** | **14,011 (#2)** | **153 (#1)** | **100 (#4)** |
| hono | 19,388 (#3) | 1,027 (#3) | 6,328 (#3) | 11,324 (#3) | 176 (#2) | 97 (#4) |
| express | 8,673 (#4) | 908 (#4) | 4,383 (#4) | 7,494 (#4) | 228 (#4) | 94 (#2) |
| sveltekit | 5,130 (#5) | 452 (#5) | 3,457 (#5) | 4,410 (#5) | 201 (#3) | 190 (#5) |
| next | 1,215 (#6) | 46 (#6) | 1,272 (#6) | 1,233 (#6) | 660 (#6) | 438 (#6) |

### Bun

c=64, 10 s runs, median of 5, cold start median of 15; 2026-10-06, CPU steal mean 0.86% (max 7%), ip_local_port_range 1024 65535; connection resets tolerated: redo up to 3 (3 tries per run, the last try kept, the same rule for every app); before every run: server under 5% of a core over 2 s before every try, 60 s at most; per-run steal max 3.09%. Each cell: req/s (place among the frameworks; a second Wisp variant is not counted against the first; a cell that is not idle, has no drain, fewer than 3 runs, steal over 10% in a run or failed requests is shown unranked with the reason).

| framework | `/` | `/list1000` | `/json-big` | `/params` | cold start ms | RSS MB after load |
|---|---|---|---|---|---|---|
| **wisp raw** | **52,112 (#1)** | **4,319 (#1)** | **9,518 (#1)** | **48,544 (#1)** | **39 (#2)** | **39 (#2)** |
| **wisp Bun.serve** | **29,184 (#3)** | **3,564 (#1)** | **7,841 (#1)** | **21,265 (#3)** | **38 (#2)** | **41 (#2)** |
| elysia | 49,965 (#2) | 1,151 (#2) | 7,471 (#2) | 23,110 (#3) | 82 (#3) | 47 (#3) |
| hono | 45,815 (#3) | 1,032 (#3) | 6,097 (#3) | 28,018 (#2) | 30 (#1) | 38 (#1) |

### Deno

c=64, 10 s runs, median of 5, cold start median of 15; 2026-10-06, CPU steal mean 0.91% (max 11%), ip_local_port_range 1024 65535; connection resets tolerated: redo up to 3 (3 tries per run, the last try kept, the same rule for every app); before every run: server under 5% of a core over 2 s before every try, 60 s at most; per-run steal max 3.9%. Each cell: req/s (place among the frameworks; a second Wisp variant is not counted against the first; a cell that is not idle, has no drain, fewer than 3 runs, steal over 10% in a run or failed requests is shown unranked with the reason).

| framework | `/` | `/list1000` | `/json-big` | `/params` | cold start ms | RSS MB after load |
|---|---|---|---|---|---|---|
| **wisp raw** | **42,533 (#2)** | **3,120 (#1)** | **7,563 (#2)** | **38,847 (#1)** | **98 (#3)** | **96 (#3)** |
| hono | 48,956 (#1) | 1,145 (#2) | 8,196 (#1) | 33,691 (#2) | 55 (#1) | 80 (#2) |
| **wisp Deno.serve** | **34,910 (#2)** | **2,773 (#1)** | **5,990 (#4)** | **21,485 (#2)** | **89 (#2)** | **63 (#1)** |
| oak | 17,344 (#4) | 1,017 (#3) | 6,079 (#4) | 12,503 (#3) | 160 (#4) | 89 (#3) |
| fresh | 22,259 (#3) | 881 (#4) | 6,495 (#3) | 8,415 (#4) | 90 (#3) | 127 (#4) |

### Where Wisp is below 3rd or behind Hono

Gap is how far behind in %: lower req/s, or higher cold start and memory. Gap to 3rd is against the framework in 3rd place (Wisp not counted).

| host | Wisp variant | metric | place | Wisp | Hono | behind Hono | behind 3rd |
|---|---|---|---|---|---|---|---|
| workerd | wisp | `/` | #2 | 6,801 | 6,880 | 1% | - |
| workerd | wisp | `/params` | #2 | 5,824 | 6,074 | 4% | - |
| workerd | wisp | cold start ms | #3 | 66 | 45 | 45% | - |
| workerd | wisp | RSS MB after load | #3 | 472 | 389 | 21% | - |
| Node | wisp raw | cold start ms | #2 | 176 | 176 | 0% | - |
| Node | wisp node:http | `/` | #3 | 16,683 | 19,388 | 14% | - |
| Node | wisp node:http | `/json-big` | #3 | 5,979 | 6,328 | 6% | - |
| Node | wisp node:http | RSS MB after load | #4 | 100 | 97 | 3% | 3% (hono) |
| Bun | wisp raw | cold start ms | #2 | 39 | 30 | 32% | - |
| Bun | wisp raw | RSS MB after load | #2 | 39 | 38 | 3% | - |
| Bun | wisp Bun.serve | `/` | #3 | 29,184 | 45,815 | 36% | - |
| Bun | wisp Bun.serve | `/params` | #3 | 21,265 | 28,018 | 24% | - |
| Bun | wisp Bun.serve | cold start ms | #2 | 38 | 30 | 27% | - |
| Bun | wisp Bun.serve | RSS MB after load | #2 | 41 | 38 | 8% | - |
| Deno | wisp raw | `/` | #2 | 42,533 | 48,956 | 13% | - |
| Deno | wisp raw | `/json-big` | #2 | 7,563 | 8,196 | 8% | - |
| Deno | wisp raw | cold start ms | #3 | 98 | 55 | 78% | - |
| Deno | wisp raw | RSS MB after load | #3 | 96 | 80 | 20% | - |
| Deno | wisp Deno.serve | `/` | #2 | 34,910 | 48,956 | 29% | - |
| Deno | wisp Deno.serve | `/json-big` | #4 | 5,990 | 8,196 | 27% | 1% (oak) |
| Deno | wisp Deno.serve | `/params` | #2 | 21,485 | 33,691 | 36% | - |
| Deno | wisp Deno.serve | cold start ms | #2 | 89 | 55 | 61% | - |
