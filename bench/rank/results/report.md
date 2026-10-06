### workerd

c=64, 10 s runs, median of 5, cold start median of 15; 2026-10-06, CPU steal mean 0.53% (max 10%), ip_local_port_range 1024 65535; connection resets tolerated: redo up to 3 (3 tries per run, the last try kept, the same rule for every app); runs redone after connection resets (requests reset): wisp / 14, itty / 14, wisp /list1000 2, hono /list1000 24, astro /list1000 5, react-router /list1000 3, hono /json-big 28, hono /params/42?q=hello%20world&x=1 18, itty /params/42?q=hello%20world&x=1 41, react-router /params/42?q=hello%20world&x=1 9; no wait-for-idle before runs (not recorded in this file); no per-run steal (not recorded in this file). Each cell: req/s (place among the frameworks; a second Wisp variant is not counted against the first; a cell that is not idle, has no drain, fewer than 3 runs, steal over 10% in a run or failed requests is shown unranked with the reason).

| framework | `/` | `/list1000` | `/json-big` | `/params` | cold start ms | RSS MB after load |
|---|---|---|---|---|---|---|
| **wisp** | **5,544 (unranked: no drain)** | **2,092 (unranked: no drain)** | **3,568 (unranked: no drain)** | **5,411 (unranked: no drain)** | **71 (#3)** | **497 (#3)** |
| hono | 6,096 (unranked: no drain) | 972 (unranked: no drain) | 3,631 (unranked: no drain) | 6,586 (unranked: no drain) | 51 (#2) | 392 (#1) |
| itty | Failed | 968 (unranked: no drain) | 3,307 (unranked: no drain) | 5,453 (unranked: no drain) | 48 (#1) | 738 (#5) |
| sveltekit | 3,008 (unranked: no drain) | 504 (unranked: no drain) | 2,529 (unranked: no drain) | 2,442 (unranked: no drain) | 76 (#4) | 1885 (#7) |
| astro | 3,369 (unranked: no drain) | 854 (unranked: no drain) | 2,667 (unranked: no drain) | 3,199 (unranked: no drain) | 106 (#6) | 463 (#2) |
| react-router | 2,428 (unranked: no drain) | 293 (unranked: no drain) | 2,122 (unranked: no drain) | 2,235 (unranked: no drain) | 87 (#5) | 739 (#6) |
| next | 428 (unranked: no drain) | 54 (unranked: no drain) | 429 (unranked: no drain) | 467 (unranked: no drain) | 375 (#7) | 680 (#4) |

### Node

c=64, 10 s runs, median of 5, cold start median of 15; 2026-10-05, CPU steal mean 0.01% (max 3%), ip_local_port_range 1024 65535; connection resets tolerated: redo up to 3 (3 tries per run, the last try kept, the same rule for every app); no wait-for-idle before runs (not recorded in this file); no per-run steal (not recorded in this file). Each cell: req/s. Not ranked: steal not read from the st column (older mark.mjs read wa).

| framework | `/` | `/list1000` | `/json-big` | `/params` | cold start ms | RSS MB after load |
|---|---|---|---|---|---|---|
| **wisp raw** | **35,201 (no drain)** | **3,354 (no drain)** | **6,817 (no drain)** | **30,911 (no drain)** | **158** | **83** |
| **wisp node:http** | **12,475 (no drain)** | **2,823 (no drain)** | **5,183 (no drain)** | **12,186 (no drain)** | **148** | **103** |
| hono | 14,149 (no drain) | 1,013 (no drain) | 4,660 (no drain) | 9,616 (no drain) | 154 | 97 |
| fastify | 17,319 (no drain) | 954 (no drain) | 5,569 (no drain) | 15,042 (no drain) | 268 | 93 |
| express | 7,405 (no drain) | 836 (no drain) | 3,191 (no drain) | 7,144 (no drain) | 207 | 94 |
| sveltekit | 3,801 (no drain) | 439 (no drain) | 2,699 (no drain) | 4,153 (no drain) | 181 | 185 |
| next | 849 (no drain) | 45 (no drain) | 883 (no drain) | 1,050 (no drain) | 645 | 445 |

### Bun

c=64, 10 s runs, median of 5, cold start median of 15; 2026-10-05, CPU steal mean 0% (max 2%), ip_local_port_range 1024 65535; connection resets tolerated: redo up to 3 (3 tries per run, the last try kept, the same rule for every app); no wait-for-idle before runs (not recorded in this file); no per-run steal (not recorded in this file). Each cell: req/s. Not ranked: steal not read from the st column (older mark.mjs read wa).

| framework | `/` | `/list1000` | `/json-big` | `/params` | cold start ms | RSS MB after load |
|---|---|---|---|---|---|---|
| **wisp raw** | **47,282 (no drain)** | **4,037 (no drain)** | **9,315 (no drain)** | **41,357 (no drain)** | **71** | **39** |
| **wisp Bun.serve** | **26,405 (no drain)** | **3,660 (no drain)** | **7,613 (no drain)** | **17,775 (no drain)** | **63** | **41** |
| hono | 37,001 (no drain) | 840 (no drain) | 6,045 (no drain) | 24,036 (no drain) | 60 | 38 |
| elysia | 43,164 (no drain) | 872 (no drain) | 6,962 (no drain) | 18,489 (no drain) | 146 | 49 |

### Deno

c=64, 10 s runs, median of 5, cold start median of 15; 2026-10-05, CPU steal mean 0% (max 2%), ip_local_port_range 1024 65535; connection resets tolerated: redo up to 3 (3 tries per run, the last try kept, the same rule for every app); no wait-for-idle before runs (not recorded in this file); no per-run steal (not recorded in this file). Each cell: req/s. Not ranked: steal not read from the st column (older mark.mjs read wa).

| framework | `/` | `/list1000` | `/json-big` | `/params` | cold start ms | RSS MB after load |
|---|---|---|---|---|---|---|
| **wisp raw** | **40,877 (no drain)** | **3,021 (no drain)** | **5,485 (no drain)** | **37,094 (no drain)** | **113** | **98** |
| **wisp Deno.serve** | **31,995 (no drain)** | **2,726 (no drain)** | **5,017 (no drain)** | **20,670 (no drain)** | **108** | **63** |
| hono | 43,146 (no drain) | 1,015 (no drain) | 7,212 (no drain) | 29,262 (no drain) | 65 | 53 |
| oak | 15,117 (no drain) | 827 (no drain) | 5,229 (no drain) | 12,748 (no drain) | 174 | 86 |
| fresh | 21,626 (no drain) | 682 (no drain) | 5,188 (no drain) | 8,517 (no drain) | 111 | 100 |

### Where Wisp is below 3rd or behind Hono

Gap is how far behind in %: lower req/s, or higher cold start and memory. Gap to 3rd is against the framework in 3rd place (Wisp not counted).

| host | Wisp variant | metric | place | Wisp | Hono | behind Hono | behind 3rd |
|---|---|---|---|---|---|---|---|
| workerd | wisp | cold start ms | #3 | 71 | 51 | 38% | - |
| workerd | wisp | RSS MB after load | #3 | 497 | 392 | 27% | - |
