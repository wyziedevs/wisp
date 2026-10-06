### workerd

No valid run (steal mean 20.11% during the run (over 10%)). The 2026-10-06 numbers are not published; the rank table is pending a valid run.

### Node

c=64, 10 s runs, median of 5, cold start median of 15; 2026-10-06, CPU steal mean 9.18% (max 65%), ip_local_port_range 1024 65535; connection resets tolerated: redo up to 3 (3 tries per run, the last try kept, the same rule for every app); before every run: server under 5% of a core over 2 s before every try, 60 s at most; per-run steal max 37.4%. Each cell: req/s (place among the frameworks; a second Wisp variant is not counted against the first; a cell that is not idle, has no drain, fewer than 3 runs, steal over 10% in a run or failed requests is shown unranked with the reason).

Provenance: Wisp 5d015b8c0ed7 built 2026-10-06; kernel 6.8.0-142-generic; node v26.10.0, bun 1.4.2, deno 2.9.7, workerd 2026-10-01; contenders @fastify/cookie 11.1.2, @hono/node-server 2.1.3, cookie-parser 1.4.7, elysia 1.4.30, express 5.2.1, fastify 5.12.5, hono 4.13.13, itty-router 5.0.24, wrangler 4.147.0, @sveltejs/adapter-cloudflare 8.0.0, @sveltejs/adapter-node 6.0.0, @sveltejs/kit 3.0.1, @sveltejs/vite-plugin-svelte 7.3.1, svelte 5.57.1, vite 8.3.3, @opennextjs/cloudflare 1.20.8, next 16.3.8, react-dom 19.3.0, react 19.3.0, @astrojs/cloudflare 14.3.3, astro 7.3.5, @cloudflare/vite-plugin 1.62.5, @react-router/dev 8.4.0, @types/react-dom 19.3.0, @types/react 19.3.0, isbot 5.2.2, react-router 8.4.0, typescript 7.0.2.

| framework | `/` | `/list1000` | `/json-big` | `/params` | cold start ms | RSS MB after load |
|---|---|---|---|---|---|---|
| **wisp raw** | **16,762 (unranked: steal 29.27% in a run)** | **2,184 (unranked: steal 15% in a run)** | **6,821 (#1)** | **25,805 (unranked: steal 17.82% in a run)** | **377 (#1)** | **83 (#1)** |
| fastify | 7,887 (unranked: steal 27.9% in a run) | 527 (unranked: steal 16.4% in a run) | 6,031 (#2) | 7,455 (unranked: steal 14.7% in a run) | 601 (#5) | 89 (#2) |
| **wisp node:http** | **7,045 (unranked: steal 30.7% in a run)** | **1,558 (unranked: steal 16.9% in a run)** | **5,087 (#3)** | **9,328 (unranked: steal 20.2% in a run)** | **342 (#1)** | **97 (#4)** |
| hono | 8,636 (unranked: steal 26.55% in a run) | 496 (unranked: steal 11.6% in a run) | 5,235 (#3) | 6,031 (unranked: steal 18.1% in a run) | 384 (#2) | 95 (#3) |
| sveltekit | 1,964 (unranked: steal 37.4% in a run) | 167 (#1) | 2,619 (#5) | 2,150 (unranked: steal 16.5% in a run) | 423 (#3) | 180 (#5) |
| express | 3,245 (unranked: steal 28.2% in a run) | 552 (unranked: steal 11.55% in a run) | 3,490 (#4) | 3,678 (unranked: steal 16.1% in a run) | 495 (#4) | 95 (#3) |
| next | 241 (unranked: steal 24.11% in a run) | 20 (#2) | 879 (#6) | 533 (unranked: steal 13% in a run) | 1453 (#6) | 251 (#6) |

### Bun

c=64, 10 s runs, median of 5, cold start median of 15; 2026-10-06, CPU steal mean 7.32% (max 29%), ip_local_port_range 1024 65535; connection resets tolerated: redo up to 3 (3 tries per run, the last try kept, the same rule for every app); before every run: server under 5% of a core over 2 s before every try, 60 s at most; per-run steal max 20.6%. Each cell: req/s (place among the frameworks; a second Wisp variant is not counted against the first; a cell that is not idle, has no drain, fewer than 3 runs, steal over 10% in a run or failed requests is shown unranked with the reason).

Provenance: Wisp 5d015b8c0ed7 built 2026-10-06; kernel 6.8.0-142-generic; node v26.10.0, bun 1.4.2, deno 2.9.7, workerd 2026-10-01; contenders @fastify/cookie 11.1.2, @hono/node-server 2.1.3, cookie-parser 1.4.7, elysia 1.4.30, express 5.2.1, fastify 5.12.5, hono 4.13.13, itty-router 5.0.24, wrangler 4.147.0, @sveltejs/adapter-cloudflare 8.0.0, @sveltejs/adapter-node 6.0.0, @sveltejs/kit 3.0.1, @sveltejs/vite-plugin-svelte 7.3.1, svelte 5.57.1, vite 8.3.3, @opennextjs/cloudflare 1.20.8, next 16.3.8, react-dom 19.3.0, react 19.3.0, @astrojs/cloudflare 14.3.3, astro 7.3.5, @cloudflare/vite-plugin 1.62.5, @react-router/dev 8.4.0, @types/react-dom 19.3.0, @types/react 19.3.0, isbot 5.2.2, react-router 8.4.0, typescript 7.0.2.

| framework | `/` | `/list1000` | `/json-big` | `/params` | cold start ms | RSS MB after load |
|---|---|---|---|---|---|---|
| **wisp raw** | **31,010 (unranked: steal 17.27% in a run)** | **3,479 (#1)** | **6,116 (#1)** | **36,699 (#1)** | **73 (#2)** | **40 (#2)** |
| **wisp Bun.serve** | **17,688 (unranked: steal 20.6% in a run)** | **2,910 (#1)** | **5,770 (unranked: steal 17.1% in a run)** | **16,694 (unranked: steal 14.1% in a run)** | **55 (#2)** | **41 (#2)** |
| hono | 24,516 (unranked: steal 17.6% in a run) | 702 (#2) | 3,921 (unranked: steal 16.2% in a run) | 16,514 (unranked: steal 18.73% in a run) | 49 (#1) | 38 (#1) |
| elysia | 26,044 (unranked: steal 16.2% in a run) | 628 (#3) | 5,011 (unranked: steal 12.1% in a run) | 12,157 (unranked: steal 17.4% in a run) | 121 (#3) | 51 (#3) |

### Deno

No valid run (steal mean 22.84% during the run (over 10%)). The 2026-10-06 numbers are not published; the rank table is pending a valid run.

### Where Wisp is below 3rd or behind Hono

Gap is how far behind in %: lower req/s, or higher cold start and memory. Gap to 3rd is against the framework in 3rd place (Wisp not counted).

| host | Wisp variant | metric | place | Wisp | Hono | behind Hono | behind 3rd |
|---|---|---|---|---|---|---|---|
| Node | wisp node:http | `/json-big` | #3 | 5,087 | 5,235 | 3% | - |
| Node | wisp node:http | RSS MB after load | #4 | 97 | 95 | 2% | 2% (express) |
| Bun | wisp raw | cold start ms | #2 | 73 | 49 | 47% | - |
| Bun | wisp raw | RSS MB after load | #2 | 40 | 38 | 5% | - |
| Bun | wisp Bun.serve | cold start ms | #2 | 55 | 49 | 11% | - |
| Bun | wisp Bun.serve | RSS MB after load | #2 | 41 | 38 | 8% | - |
