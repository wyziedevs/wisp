## Throughput (req/s, median of 5 x 10 s; higher is better; ~ marks a cell where host steal stayed above 8% in some run)

### `/` plaintext

| Rank | Framework | Pass 1 | Pass 2 | Mean | Pass gap | Rank p1/p2 | p99 ms p1/p2 |
|---|---|---|---|---|---|---|---|
| 1 | Actix Web | 74,080 | 82,670 | 78,375 | 11.0% **FLAG** | 1/2 | 4.97 / 5.39 |
| 2 | Axum | 60,893 | 79,731 | 70,312 | 26.8% **FLAG** | 2/3 | 5.33 / 4.24 |
| 3 | Wisp | 44,490~ | 87,608 | 66,049 | 65.3% **FLAG** | 4/1 | 10.16 / 3.84 |
| 4 | Hono (Bun) | 45,243~ | 51,454 | 48,349 | 12.8% **FLAG** | 3/4 | 7.55 / 6.17 |
| 5 | ASP.NET Core | 40,234~ | 49,794 | 45,014 | 21.2% **FLAG** | 5/5 | 7.18 / 6.15 |
| 6 | Go Gin | 29,417 | 29,779 | 29,598 | 1.2% | 6/6 | 11.03 / 10.81 |
| 7 | Fastify | 25,933 | 22,015~ | 23,974 | 16.3% **FLAG** | 7/7 | 7.24 / 9.18 |
| 8 | Express | 15,356~ | 16,136 | 15,746 | 5.0% | 8/8 | 11.23 / 10.52 |
| 9 | FastAPI | 10,554 | 8,107 | 9,331 | 26.2% **FLAG** | 9/9 | 12.44 / 19.47 |
| 10 | Spring Boot | 8,999 | 7,787 | 8,393 | 14.4% **FLAG** | 11/10 | 40.02 / 57.69 |
| 11 | SvelteKit | 9,479 | 6,616 | 8,048 | 35.6% **FLAG** | 10/11 | 20.8 / 36.06 |
| 12 | Next.js | 1,217 | 1,919 | 1,568 | 44.8% **FLAG** | 12/12 | 197.5 / 85.61 |

### `/json`

| Rank | Framework | Pass 1 | Pass 2 | Mean | Pass gap | Rank p1/p2 | p99 ms p1/p2 |
|---|---|---|---|---|---|---|---|
| 1 | Wisp | 60,036~ | 91,569 | 75,803 | 41.6% **FLAG** | 3/1 | 6.54 / 3.64 |
| 2 | Actix Web | 81,244 | 57,353~ | 69,299 | 34.5% **FLAG** | 1/3 | 5.4 / 7.22 |
| 3 | Axum | 75,678 | 55,014~ | 65,346 | 31.6% **FLAG** | 2/4 | 4.01 / 5.98 |
| 4 | ASP.NET Core | 31,570~ | 64,888 | 48,229 | 69.1% **FLAG** | 5/2 | 9.98 / 3.48 |
| 5 | Hono (Bun) | 39,037~ | 44,219 | 41,628 | 12.4% **FLAG** | 4/5 | 8.18 / 6.7 |
| 6 | Go Gin | 30,038~ | 23,934~ | 26,986 | 22.6% **FLAG** | 6/6 | 10.02 / 13.54 |
| 7 | Fastify | 24,829 | 21,731~ | 23,280 | 13.3% **FLAG** | 7/7 | 6.95 / 9.16 |
| 8 | Spring Boot | 16,131 | 15,647 | 15,889 | 3.0% | 8/8 | 14.04 / 15.03 |
| 9 | Express | 14,389~ | 13,733~ | 14,061 | 4.7% | 9/9 | 12.82 / 11.96 |
| 10 | SvelteKit | 8,634 | 8,485 | 8,560 | 1.7% | 11/10 | 25.95 / 25.06 |
| 11 | FastAPI | 9,219 | 6,924~ | 8,072 | 28.4% **FLAG** | 10/11 | 14.57 / 22.72 |
| 12 | Next.js | 1,741 | 1,717 | 1,729 | 1.4% | 12/12 | 107.44 / 98.28 |

### `/params/42?q=hello` + cookie

| Rank | Framework | Pass 1 | Pass 2 | Mean | Pass gap | Rank p1/p2 | p99 ms p1/p2 |
|---|---|---|---|---|---|---|---|
| 1 | Wisp | 47,909~ | 89,335 | 68,622 | 60.4% **FLAG** | 4/1 | 8.62 / 3.63 |
| 2 | Actix Web | 78,418 | 51,488~ | 64,953 | 41.5% **FLAG** | 1/3 | 4.28 / 7.09 |
| 3 | Axum | 54,361 | 51,584~ | 52,973 | 5.2% **FLAG** | 2/2 | 5.4 / 5.98 |
| 4 | Hono (Bun) | 52,244 | 25,635~ | 38,940 | 68.3% **FLAG** | 3/5 | 4.15 / 13.46 |
| 5 | ASP.NET Core | 26,068~ | 43,992 | 35,030 | 51.2% **FLAG** | 7/4 | 13.23 / 5.71 |
| 6 | Go Gin | 29,756 | 21,495~ | 25,626 | 32.2% **FLAG** | 5/6 | 9.76 / 16.19 |
| 7 | Fastify | 27,939 | 14,509~ | 21,224 | 63.3% **FLAG** | 6/9 | 5.64 / 19.9 |
| 8 | Express | 11,165~ | 17,924 | 14,545 | 46.5% **FLAG** | 9/7 | 16.79 / 7.68 |
| 9 | Spring Boot | 12,315~ | 16,128 | 14,222 | 26.8% **FLAG** | 8/8 | 22.06 / 14.26 |
| 10 | FastAPI | 5,811 | 5,019~ | 5,415 | 14.6% **FLAG** | 10/11 | 23.87 / 30.77 |
| 11 | SvelteKit | 4,946~ | 5,209~ | 5,078 | 5.2% **FLAG** | 11/10 | 50.61 / 46 |
| 12 | Next.js | 1,360 | 1,481 | 1,421 | 8.5% **FLAG** | 12/12 | 151.57 / 171.94 |

### `/list` 1000 items

| Rank | Framework | Pass 1 | Pass 2 | Mean | Pass gap | Rank p1/p2 | p99 ms p1/p2 |
|---|---|---|---|---|---|---|---|
| 1 | Actix Web | 18,947 | 11,399~ | 15,173 | 49.7% **FLAG** | 1/2 | 10.05 / 21.02 |
| 2 | Axum | 13,497~ | 16,727~ | 15,112 | 21.4% **FLAG** | 2/1 | 16.42 / 12.18 |
| 3 | Wisp | 6,245~ | 9,045~ | 7,645 | 36.6% **FLAG** | 3/3 | 32.1 / 14.49 |
| 4 | Spring Boot | 2,282~ | 3,814~ | 3,048 | 50.3% **FLAG** | 4/4 | 136.76 / 81.88 |
| 5 | ASP.NET Core | 1,761~ | 2,161~ | 1,961 | 20.4% **FLAG** | 5/5 | 84.3 / 72.21 |
| 6 | Fastify | 1,502 | 1,282 | 1,392 | 15.8% **FLAG** | 6/7 | 107.57 / 111.09 |
| 7 | Express | 1,093 | 1,579 | 1,336 | 36.4% **FLAG** | 8/6 | 130.45 / 92.4 |
| 8 | FastAPI | 1,077 | 1,240 | 1,159 | 14.1% **FLAG** | 9/8 | 178.16 / 148 |
| 9 | Hono (Bun) | 1,410 | 704~ | 1,057 | 66.8% **FLAG** | 7/10 | 90.78 / 173.5 |
| 10 | SvelteKit | 518 | 771 | 645 | 39.3% **FLAG** | 11/9 | 318.6 / 215.97 |
| 11 | Go Gin | 570 | 410~ | 490 | 32.7% **FLAG** | 10/11 | 373.25 / 567.59 |
| 12 | Next.js | 37 | 32 | 35 | 14.5% **FLAG** | 12/12 | 3400.52 / 3716.94 |

### `/json-big` 200 objects

| Rank | Framework | Pass 1 | Pass 2 | Mean | Pass gap | Rank p1/p2 | p99 ms p1/p2 |
|---|---|---|---|---|---|---|---|
| 1 | Wisp | 12,207~ | 16,607 | 14,407 | 30.5% **FLAG** | 2/1 | 18.85 / 9.85 |
| 2 | Actix Web | 14,028 | 12,629~ | 13,329 | 10.5% **FLAG** | 1/2 | 9.52 / 12.69 |
| 3 | Axum | 9,576 | 12,041~ | 10,809 | 22.8% **FLAG** | 4/3 | 15.31 / 10.63 |
| 4 | Hono (Bun) | 10,445 | 5,807~ | 8,126 | 57.1% **FLAG** | 3/7 | 14.56 / 30.81 |
| 5 | Fastify | 7,512~ | 7,632 | 7,572 | 1.6% | 6/4 | 21.06 / 20.21 |
| 6 | ASP.NET Core | 5,616~ | 7,457 | 6,537 | 28.2% **FLAG** | 7/5 | 28.39 / 23.42 |
| 7 | Spring Boot | 7,631~ | 5,272~ | 6,452 | 36.6% **FLAG** | 5/9 | 51.21 / 57.65 |
| 8 | Express | 4,857~ | 7,142 | 6,000 | 38.1% **FLAG** | 9/6 | 36.49 / 20.2 |
| 9 | Go Gin | 5,063~ | 4,840~ | 4,952 | 4.5% | 8/10 | 39.14 / 40.72 |
| 10 | SvelteKit | 3,743 | 5,339 | 4,541 | 35.1% **FLAG** | 10/8 | 41.38 / 27.23 |
| 11 | Next.js | 1,450 | 942 | 1,196 | 42.5% **FLAG** | 11/11 | 123.79 / 186.56 |
| 12 | FastAPI | 509 | 536 | 523 | 5.2% **FLAG** | 12/12 | 298.77 / 263.46 |

## Rank summary (by mean of both passes; 1 is fastest)

| Framework | plaintext | json | params | list | json-big | Sum |
|---|---|---|---|---|---|---|
| Actix Web | 1 | 2 | 2 | 1 | 2 | 8 |
| Wisp | 3 | 1 | 1 | 3 | 1 | 9 |
| Axum | 2 | 3 | 3 | 2 | 3 | 13 |
| ASP.NET Core | 5 | 4 | 5 | 5 | 6 | 25 |
| Hono (Bun) | 4 | 5 | 4 | 9 | 4 | 26 |
| Fastify | 7 | 7 | 7 | 6 | 5 | 32 |
| Go Gin | 6 | 6 | 6 | 11 | 9 | 38 |
| Spring Boot | 10 | 8 | 9 | 4 | 7 | 38 |
| Express | 8 | 9 | 8 | 7 | 8 | 40 |
| FastAPI | 9 | 11 | 10 | 8 | 12 | 50 |
| SvelteKit | 11 | 10 | 11 | 10 | 10 | 52 |
| Next.js | 12 | 12 | 12 | 12 | 11 | 59 |

## Cold start, spawn to first 200 on `/` (ms; lower is better)

| Rank | Framework | Pass 1 | Pass 2 | Mean | Pass gap |
|---|---|---|---|---|---|
| 1 | Actix Web | 25 | 25 | 25 | 0.0% |
| 2 | Axum | 56 | 25 | 41 | 76.5% (differs; see notes) |
| 3 | Wisp | 55 | 33 | 44 | 50.0% (differs; see notes) |
| 4 | Go Gin | 57 | 48 | 53 | 17.1% (differs; see notes) |
| 5 | Hono (Bun) | 82 | 96 | 89 | 15.7% (differs; see notes) |
| 6 | SvelteKit | 270 | 473 | 372 | 54.6% (differs; see notes) |
| 7 | Express | 512 | 281 | 397 | 58.3% (differs; see notes) |
| 8 | Fastify | 452 | 566 | 509 | 22.4% (differs; see notes) |
| 9 | ASP.NET Core | 733 | 548 | 641 | 28.9% (differs; see notes) |
| 10 | FastAPI | 1,217 | 908 | 1,063 | 29.1% (differs; see notes) |
| 11 | Next.js | 1,088 | 1,285 | 1,187 | 16.6% (differs; see notes) |
| 12 | Spring Boot | 4,493 | 5,268 | 4,881 | 15.9% (differs; see notes) |

## RSS after load (all processes of the server) (MB; lower is better)

| Rank | Framework | Pass 1 | Pass 2 | Mean | Pass gap |
|---|---|---|---|---|---|
| 1 | Wisp | 4 | 4 | 4 | 0.0% |
| 2 | Actix Web | 5 | 5 | 5 | 0.0% |
| 3 | Axum | 6 | 7 | 7 | 15.4% (differs; see notes) |
| 4 | Go Gin | 24 | 24 | 24 | 0.0% |
| 5 | Hono (Bun) | 88 | 80 | 84 | 9.5% (differs; see notes) |
| 6 | ASP.NET Core | 96 | 93 | 95 | 3.2% |
| 7 | FastAPI | 140 | 140 | 140 | 0.0% |
| 8 | Fastify | 284 | 376 | 330 | 27.9% (differs; see notes) |
| 9 | Express | 282 | 496 | 389 | 55.0% (differs; see notes) |
| 10 | Spring Boot | 571 | 567 | 569 | 0.7% |
| 11 | Next.js | 1,137 | 1,129 | 1,133 | 0.7% |
| 12 | SvelteKit | 1,957 | 1,921 | 1,939 | 1.9% |

## Cells where the two passes differ by more than 5%

- Actix Web plaintext: 11.0%
- Axum plaintext: 26.8%
- Wisp plaintext: 65.3%
- Hono (Bun) plaintext: 12.8%
- ASP.NET Core plaintext: 21.2%
- Fastify plaintext: 16.3%
- FastAPI plaintext: 26.2%
- Spring Boot plaintext: 14.4%
- SvelteKit plaintext: 35.6%
- Next.js plaintext: 44.8%
- Wisp json: 41.6%
- Actix Web json: 34.5%
- Axum json: 31.6%
- ASP.NET Core json: 69.1%
- Hono (Bun) json: 12.4%
- Go Gin json: 22.6%
- Fastify json: 13.3%
- FastAPI json: 28.4%
- Wisp params: 60.4%
- Actix Web params: 41.5%
- Axum params: 5.2%
- Hono (Bun) params: 68.3%
- ASP.NET Core params: 51.2%
- Go Gin params: 32.2%
- Fastify params: 63.3%
- Express params: 46.5%
- Spring Boot params: 26.8%
- FastAPI params: 14.6%
- SvelteKit params: 5.2%
- Next.js params: 8.5%
- Actix Web list: 49.7%
- Axum list: 21.4%
- Wisp list: 36.6%
- Spring Boot list: 50.3%
- ASP.NET Core list: 20.4%
- Fastify list: 15.8%
- Express list: 36.4%
- FastAPI list: 14.1%
- Hono (Bun) list: 66.8%
- SvelteKit list: 39.3%
- Go Gin list: 32.7%
- Next.js list: 14.5%
- Wisp json-big: 30.5%
- Actix Web json-big: 10.5%
- Axum json-big: 22.8%
- Hono (Bun) json-big: 57.1%
- ASP.NET Core json-big: 28.2%
- Spring Boot json-big: 36.6%
- Express json-big: 38.1%
- SvelteKit json-big: 35.1%
- Next.js json-big: 42.5%
- FastAPI json-big: 5.2%
