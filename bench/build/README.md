# Build time and dev reload

Pure data and a generator. `measure.mjs` runs on the Linux box and writes `results/<contender>.json`; `report.mjs` derives
every rank and the markdown table from those files (`node report.mjs > results/report.md`). `node --test bench/build/lib.test.mjs` tests the generator.

`gen.sh` and `run.sh` (older) time one Wisp app with N generated routes; they are not part of this comparison.

## What is measured

Same machine, method and limits for Wisp, SvelteKit, Next.js, Nuxt, Hono (node, bun), Express, Fastify, Axum, Actix. The apps are the
`bench/tfb/*` ones (the plaintext and json routes), built the way each one's docs say. SvelteKit, Next.js, Nuxt and Wisp also get
`pages/<contender>/`, one `/hello` page (`<h1>Hello, World!</h1>`) copied over the app before every measurement, so a markup edit
exists; the page is part of the cold and warm builds of those four.

| cell | what |
|---|---|
| `cold` | clean build output and caches removed (`target`, `.svelte-kit`, `build`, `.next`, `.nuxt`, `.output`, `node_modules/.cache`), dependencies already downloaded (`cargo fetch`, `npm install` run first, not timed). Rust: dependencies compile inside the timing. |
| `warm_noop` | the same build command again, nothing changed |
| `warm_edit` / `warm_edit_markup` | one file edited (a version tag after the first `Hello, World!` of the plaintext route, or of `/hello`), then the build command |
| `dev_logic` / `dev_markup` | the dev server is running and warm; a file is edited; the time until `GET` returns the new text, polled every 5 ms (node `http`, not curl: a process per poll would add its own load) |
| artifact | bytes of the binary or output dir (node_modules not counted); source only for contenders without a build |

Build commands: `cargo build --release --offline` (Wisp, Axum, Actix; the app's own release profile), `npx vite build`, `npx next build`,
`npx nuxt build`. Hono, Express, Fastify run their source: no build cells. Dev: `wisp dev`, `vite dev`, `next dev`, `nuxt dev`, `tsx watch`
(Hono Node template), `bun --hot` (Hono Bun template); Express, Fastify, Axum and Actix have no dev server in their docs, so no dev cell.
`CARGO_BUILD_JOBS` is unset for every contender; `nproc`, tool versions and `ip_local_port_range` are in each file.

## Validity

Every timed run, install and dev session holds `flock /tmp/wisp-bench.lock` (one hold per single run; installs separately). Inside the lock: drain (machine under 5% of one core busy,
2 s windows, up to 180 s), read `/proc/stat` (fields 2..9, steal is the last), run, read it again. Steal over 10% of all CPU time during
the run makes it invalid; it is retried up to 3 times, then the cell publishes nothing (`na`). Ranks need 3 or more valid runs, are by
median, and ties share a rank. A stored flag is never read: `cellWhy` in `lib.mjs` derives validity from the data.

## Run

```
node --test bench/build/lib.test.mjs
scp the repo, then on the box:   node bench/build/measure.mjs run wisp --runs 3
node bench/build/report.mjs > bench/build/results/report.md
```

`dev` for Wisp needs the `wisp` CLI at `/root/bb/wisp` (`cargo build --release -p wisp-web`). Tokens for app authors are not measured
here: see `bench/tokens/results.json`.
