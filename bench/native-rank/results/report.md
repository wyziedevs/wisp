## Throughput (req/s, median of 5 x 10 s per pass; higher is better)

`~` = host steal stayed above the gate in that pass. Final = median of the clean passes (needs at least two, else the cell is unranked). Status: ok = two clean passes within 5%; **no agree** = they never did (the shared host, see README).

### `/` plaintext

| Rank | Framework | Pass 1 | Pass 2 | Pass 3 | Pass 4 | Pass 5 | Final | Status |
|---|---|---|---|---|---|---|---|---|
| 1 | Wisp | 44,490~ | 87,608 | 68,924~ | 90,823 | 77,538~ | 89,216 | ok |
| 2 | Actix Web | 74,080 | 82,670 | 82,692 | 92,849 | 84,963 | 82,692 | ok |
| 3 | Axum | 60,893 | 79,731 | 82,640 | 86,630 | 76,142 | 79,731 | ok |
| 4 | ASP.NET Core | 40,234~ | 49,794 | - | 60,309 | 60,594 | 60,309 | ok |
| 5 | Go Gin | 29,417 | 29,779 | - | 38,913 | 27,864~ | 29,779 | ok |
| 6 | FastAPI | 10,554 | 8,107 | - | 6,934~ | 8,295~ | 9,331 | **no agree** |
| 7 | Spring Boot | 8,999 | 7,787 | 12,900~ | 4,091~ | 5,251~ | 8,393 | **no agree** |
| 8 | SvelteKit | 9,479 | 6,616 | - | 4,668~ | 6,129~ | 8,048 | **no agree** |
| 9 | Next.js | 1,217 | 1,919 | - | 786~ | 591~ | 1,568 | **no agree** |
| - | Express | 15,356~ | 16,136 | - | 14,266~ | 12,164~ | - | unranked: too few clean passes |
| - | Fastify | 25,933 | 22,015~ | 13,977~ | 15,914~ | 19,007~ | - | unranked: too few clean passes |
| - | Hono (Bun) | 45,243~ | 51,454 | - | 40,239~ | 46,429~ | - | unranked: too few clean passes |

### `/json`

| Rank | Framework | Pass 1 | Pass 2 | Pass 3 | Pass 4 | Pass 5 | Final | Status |
|---|---|---|---|---|---|---|---|---|
| 1 | Wisp | 60,036~ | 91,569 | 48,710~ | 84,661~ | 72,778 | 82,174 | **no agree** |
| 2 | Axum | 75,678 | 55,014~ | 92,089 | 84,997 | 67,527 | 80,338 | **no agree** |
| 3 | Actix Web | 81,244 | 57,353~ | 62,880~ | 77,064 | 80,252 | 80,252 | ok |
| 4 | ASP.NET Core | 31,570~ | 64,888 | - | 67,032 | 52,888 | 64,888 | ok |
| 5 | Spring Boot | 16,131 | 15,647 | 13,797~ | 11,829~ | 9,952~ | 15,889 | ok |
| 6 | SvelteKit | 8,634 | 8,485 | - | 4,212~ | 6,189~ | 8,560 | ok |
| 7 | Next.js | 1,741 | 1,717 | - | 709~ | 744~ | 1,729 | ok |
| - | Express | 14,389~ | 13,733~ | - | 15,688~ | 12,283~ | - | unranked: too few clean passes |
| - | FastAPI | 9,219 | 6,924~ | - | 4,981~ | 5,434~ | - | unranked: too few clean passes |
| - | Fastify | 24,829 | 21,731~ | 17,570~ | 14,689~ | 23,199~ | - | unranked: too few clean passes |
| - | Go Gin | 30,038~ | 23,934~ | - | 39,274 | 37,194~ | - | unranked: too few clean passes |
| - | Hono (Bun) | 39,037~ | 44,219 | - | 40,524~ | 32,479~ | - | unranked: too few clean passes |

### `/params/42?q=hello` + cookie

| Rank | Framework | Pass 1 | Pass 2 | Pass 3 | Pass 4 | Pass 5 | Final | Status |
|---|---|---|---|---|---|---|---|---|
| 1 | Actix Web | 78,418 | 51,488~ | 62,492~ | 94,088 | 64,049 | 78,418 | **no agree** |
| 2 | Axum | 54,361 | 51,584~ | 65,284 | 81,241 | 73,136 | 69,210 | **no agree** |
| 3 | ASP.NET Core | 26,068~ | 43,992 | - | 57,972 | 49,586 | 49,586 | **no agree** |
| 4 | Go Gin | 29,756 | 21,495~ | - | 33,030 | 30,233~ | 31,393 | **no agree** |
| 5 | Express | 11,165~ | 17,924 | - | 11,013~ | 16,763 | 17,344 | **no agree** |
| 6 | Next.js | 1,360 | 1,481 | - | 447~ | 488~ | 1,421 | **no agree** |
| - | FastAPI | 5,811 | 5,019~ | - | 3,766~ | 5,724~ | - | unranked: too few clean passes |
| - | Fastify | 27,939 | 14,509~ | 13,409~ | 15,432~ | 22,323~ | - | unranked: too few clean passes |
| - | Hono (Bun) | 52,244 | 25,635~ | - | 31,422~ | 24,622~ | - | unranked: too few clean passes |
| - | Spring Boot | 12,315~ | 16,128 | 12,939~ | 13,835~ | 10,943~ | - | unranked: too few clean passes |
| - | SvelteKit | 4,946~ | 5,209~ | - | 3,842~ | 5,428~ | - | unranked: too few clean passes |
| - | Wisp | 47,909~ | 89,335 | 54,323~ | 62,910~ | 75,012~ | - | unranked: too few clean passes |

### `/list` 1000 items

| Rank | Framework | Pass 1 | Pass 2 | Pass 3 | Pass 4 | Pass 5 | Final | Status |
|---|---|---|---|---|---|---|---|---|
| 1 | Actix Web | 18,947 | 11,399~ | 10,175~ | 25,386 | 14,179 | 18,947 | **no agree** |
| 2 | Fastify | 1,502 | 1,282 | 791~ | 1,415 | 909~ | 1,415 | **no agree** |
| 3 | Express | 1,093 | 1,579 | - | 794~ | 1,058~ | 1,336 | **no agree** |
| 4 | FastAPI | 1,077 | 1,240 | - | 634~ | 954 | 1,077 | **no agree** |
| 5 | Go Gin | 570 | 410~ | - | 942 | 753 | 753 | **no agree** |
| 6 | SvelteKit | 518 | 771 | - | 422 | 371~ | 518 | **no agree** |
| 7 | Next.js | 37 | 32 | - | 18~ | 20~ | 35 | **no agree** |
| - | ASP.NET Core | 1,761~ | 2,161~ | - | 4,241 | 2,261~ | - | unranked: too few clean passes |
| - | Axum | 13,497~ | 16,727~ | 12,886~ | 23,651 | 12,866~ | - | unranked: too few clean passes |
| - | Hono (Bun) | 1,410 | 704~ | - | 813~ | 737~ | - | unranked: too few clean passes |
| - | Spring Boot | 2,282~ | 3,814~ | 2,078~ | 3,568~ | 1,872~ | - | unranked: too few clean passes |
| - | Wisp | 6,245~ | 9,045~ | 5,859~ | 6,581~ | 9,229 | - | unranked: too few clean passes |

### `/json-big` 200 objects

| Rank | Framework | Pass 1 | Pass 2 | Pass 3 | Pass 4 | Pass 5 | Final | Status |
|---|---|---|---|---|---|---|---|---|
| 1 | Actix Web | 14,028 | 12,629~ | 9,100~ | 17,534 | 14,104 | 14,104 | ok |
| 2 | Axum | 9,576 | 12,041~ | 10,565~ | 12,556 | 10,277~ | 11,066 | **no agree** |
| 3 | ASP.NET Core | 5,616~ | 7,457 | - | 10,489 | 5,309~ | 8,973 | **no agree** |
| 4 | Fastify | 7,512~ | 7,632 | 6,111~ | 7,656 | 5,194~ | 7,644 | ok |
| 5 | Go Gin | 5,063~ | 4,840~ | - | 5,569 | 6,271 | 5,920 | **no agree** |
| 6 | SvelteKit | 3,743 | 5,339 | - | 2,721~ | 3,069~ | 4,541 | **no agree** |
| 7 | Next.js | 1,450 | 942 | - | 499~ | 549~ | 1,196 | **no agree** |
| 8 | FastAPI | 509 | 536 | - | 289~ | 484 | 509 | **no agree** |
| - | Express | 4,857~ | 7,142 | - | 3,944~ | 4,411~ | - | unranked: too few clean passes |
| - | Hono (Bun) | 10,445 | 5,807~ | - | 7,335~ | 6,120~ | - | unranked: too few clean passes |
| - | Spring Boot | 7,631~ | 5,272~ | 5,883~ | 4,581~ | 4,416~ | - | unranked: too few clean passes |
| - | Wisp | 12,207~ | 16,607 | 9,752~ | 15,288~ | 15,172~ | - | unranked: too few clean passes |

## Rank summary (by final value; 1 is fastest)

| Framework | plaintext | json | params | list | json-big | Sum (ranked cells) | Ranked cells |
|---|---|---|---|---|---|---|---|
| Actix Web | 2 | 3 | 1 | 1 | 1 | 8 | 5 |
| Next.js | 9 | 7 | 6 | 7 | 7 | 36 | 5 |
| Axum | 3 | 2 | 2 | - | 2 | 9 | 4 |
| ASP.NET Core | 4 | 4 | 3 | - | 3 | 14 | 4 |
| Go Gin | 5 | - | 4 | 5 | 5 | 19 | 4 |
| SvelteKit | 8 | 6 | - | 6 | 6 | 26 | 4 |
| FastAPI | 6 | - | - | 4 | 8 | 18 | 3 |
| Wisp | 1 | 1 | - | - | - | 2 | 2 |
| Fastify | - | - | - | 2 | 4 | 6 | 2 |
| Express | - | - | 5 | 3 | - | 8 | 2 |
| Spring Boot | 7 | 5 | - | - | - | 12 | 2 |
| Hono (Bun) | - | - | - | - | - | 0 | 0 |

## Per pass ranks (what each pass said on its own)

| Framework | Pass 1: plaintext json params list json-big | Pass 2: plaintext json params list json-big | Pass 3: plaintext json params list json-big | Pass 4: plaintext json params list json-big | Pass 5: plaintext json params list json-big |
|---|---|---|---|---|---|
| Actix Web | 1 1 1 1 1 | 2 3 3 2 2 | 1 2 2 2 3 | 1 3 1 1 1 | 1 1 3 1 2 |
| Next.js | 12 12 12 12 11 | 12 12 12 12 11 | - - - - - | 12 12 12 12 11 | 12 12 12 12 11 |
| Axum | 2 2 2 2 4 | 3 4 2 1 3 | 2 1 1 1 1 | 3 1 2 2 3 | 3 3 2 2 3 |
| ASP.NET Core | 5 5 7 5 7 | 5 2 4 5 5 | - - - - - | 4 4 4 4 4 | 4 4 4 4 6 |
| Go Gin | 6 6 5 10 8 | 6 6 6 11 10 | - - - - - | 6 6 5 7 7 | 6 5 5 9 4 |
| SvelteKit | 10 11 11 11 10 | 11 10 10 9 8 | - - - - - | 10 11 10 11 10 | 10 10 11 11 10 |
| FastAPI | 9 10 10 9 12 | 9 11 11 8 12 | - - - - - | 9 10 11 10 12 | 9 11 10 7 12 |
| Wisp | 4 3 4 3 2 | 1 1 1 3 1 | 3 3 3 3 2 | 2 2 3 3 2 | 2 2 1 3 1 |
| Fastify | 7 7 6 6 6 | 7 7 9 7 4 | 4 4 4 5 4 | 7 8 7 6 5 | 7 7 7 8 7 |
| Express | 8 9 9 8 9 | 8 9 7 6 6 | - - - - - | 8 7 9 9 9 | 8 8 8 6 9 |
| Spring Boot | 11 8 8 4 5 | 10 8 8 4 9 | 5 5 5 4 5 | 11 9 8 5 8 | 11 9 9 5 8 |
| Hono (Bun) | 3 4 3 7 3 | 4 5 5 10 7 | - - - - - | 5 5 6 8 6 | 5 6 6 10 5 |

## CPU per request (server CPU microseconds, user+system, both cores; lower is better; rank in brackets)

| Framework | plaintext | json | params | list | json-big | Sum of ranks |
|---|---|---|---|---|---|---|
| Wisp | 18.7 (1) | 19.9 (1) | 23.2 (1) | 257.4 (3) | 130.4 (2) | 8 |
| Actix Web | 19.7 (2) | 22.1 (2) | 24.6 (3) | 108.8 (1) | 127.7 (1) | 9 |
| Axum | 22.7 (3) | 24.2 (3) | 24.5 (2) | 116.4 (2) | 170.6 (3) | 13 |
| ASP.NET Core | 32.0 (4) | 33.0 (4) | 35.6 (4) | 620.0 (4) | 275.1 (4) | 20 |
| Go Gin | 58.6 (6) | 50.7 (5) | 61.0 (5) | 2366.3 (8) | 326.9 (7) | 31 |
| Hono (Bun) | 42.6 (5) | 52.3 (6) | 69.3 (6) | 2611.5 (9) | 299.3 (5) | 31 |
| Fastify | 112.9 (7) | 110.7 (7) | 108.7 (7) | 1821.1 (6) | 322.1 (6) | 33 |
| Express | 150.3 (8) | 144.3 (8) | 149.8 (8) | 2225.1 (7) | 474.9 (9) | 40 |
| Spring Boot | 435.6 (11) | 173.3 (9) | 157.2 (9) | 807.2 (5) | 436.4 (8) | 42 |
| FastAPI | 263.9 (9) | 380.4 (10) | 437.3 (10) | 2643.6 (10) | 5647.5 (12) | 51 |
| SvelteKit | 376.8 (10) | 398.4 (11) | 441.0 (11) | 5114.2 (11) | 693.1 (10) | 53 |
| Next.js | 2992.7 (12) | 2788.3 (12) | 4352.9 (12) | 156851.3 (12) | 3884.3 (11) | 59 |

## Cold start, spawn to first 200 on `/` (ms; lower is better; median of the passes)

| Rank | Framework | Pass 1 | Pass 2 | Pass 3 | Pass 4 | Pass 5 | Median |
|---|---|---|---|---|---|---|---|
| 1 | Actix Web | 25 | 25 | 24 | 33 | 20 | 25 |
| 2 | Axum | 56 | 25 | 27 | 16 | 154 | 27 |
| 3 | Wisp | 55 | 33 | 33 | 37 | 26 | 33 |
| 4 | Go Gin | 57 | 48 | - | 54 | 49 | 52 |
| 5 | Hono (Bun) | 82 | 96 | - | 110 | 80 | 89 |
| 6 | SvelteKit | 270 | 473 | - | 455 | 342 | 399 |
| 7 | Express | 512 | 281 | - | 464 | 463 | 464 |
| 8 | Fastify | 452 | 566 | 861 | 769 | 351 | 566 |
| 9 | ASP.NET Core | 733 | 548 | - | 528 | 781 | 641 |
| 10 | FastAPI | 1,217 | 908 | - | 1,756 | 1,258 | 1,238 |
| 11 | Next.js | 1,088 | 1,285 | - | 1,800 | 1,935 | 1,543 |
| 12 | Spring Boot | 4,493 | 5,268 | 11,834 | 10,730 | 12,085 | 10,730 |

## RSS after load, all processes of the server (MB; lower is better; median of the passes)

| Rank | Framework | Pass 1 | Pass 2 | Pass 3 | Pass 4 | Pass 5 | Median |
|---|---|---|---|---|---|---|---|
| 1 | Wisp | 4 | 4 | 4 | 4 | 4 | 4 |
| 2 | Actix Web | 5 | 5 | 5 | 5 | 5 | 5 |
| 3 | Axum | 6 | 7 | 7 | 6 | 6 | 6 |
| 4 | Go Gin | 24 | 24 | - | 23 | 24 | 24 |
| 5 | Hono (Bun) | 88 | 80 | - | 87 | 83 | 85 |
| 6 | ASP.NET Core | 96 | 93 | - | 96 | 90 | 95 |
| 7 | FastAPI | 140 | 140 | - | 140 | 140 | 140 |
| 8 | Fastify | 284 | 376 | 485 | 479 | 266 | 376 |
| 9 | Express | 282 | 496 | - | 491 | 492 | 492 |
| 10 | Spring Boot | 571 | 567 | 588 | 568 | 543 | 568 |
| 11 | Next.js | 1,137 | 1,129 | - | 1,064 | 1,095 | 1,112 |
| 12 | SvelteKit | 1,957 | 1,921 | - | 1,248 | 1,692 | 1,807 |

## Wisp outside the top 3

- `/params/42?q=hello` + cookie: unranked (too few clean passes)
- `/list` 1000 items: unranked (too few clean passes)
- `/json-big` 200 objects: unranked (too few clean passes)

## Cells where no two clean passes agree within 5%

- FastAPI plaintext
- Spring Boot plaintext
- SvelteKit plaintext
- Next.js plaintext
- Express plaintext
- Fastify plaintext
- Hono (Bun) plaintext
- Wisp json
- Axum json
- Express json
- FastAPI json
- Fastify json
- Go Gin json
- Hono (Bun) json
- Actix Web params
- Axum params
- ASP.NET Core params
- Go Gin params
- Express params
- Next.js params
- FastAPI params
- Fastify params
- Hono (Bun) params
- Spring Boot params
- SvelteKit params
- Wisp params
- Actix Web list
- Fastify list
- Express list
- FastAPI list
- Go Gin list
- SvelteKit list
- Next.js list
- ASP.NET Core list
- Axum list
- Hono (Bun) list
- Spring Boot list
- Wisp list
- Axum json-big
- ASP.NET Core json-big
- Go Gin json-big
- SvelteKit json-big
- Next.js json-big
- FastAPI json-big
- Express json-big
- Hono (Bun) json-big
- Spring Boot json-big
- Wisp json-big
