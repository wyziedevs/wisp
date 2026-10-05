#!/usr/bin/env node
// Native ranking bench driver. Run ON the Linux bench host (see README); run.sh wraps it in flock.
//   node run.mjs build  [--fw a,b]
//   node run.mjs verify [--fw a,b]                      same bytes from every framework?
//   node run.mjs bench --pass N [--fw a,b] [--routes plaintext,json,...] [--runs 5] [--secs 10]
// Method: server pinned to cores 0-1, oha pinned to cores 2-3, c=64, 5 s warm-up per route, then
// `runs` timed runs of `secs`; the median (by req/s) is kept. Also cold start (spawn to first 200
// on /) and RSS summed over the server's whole process session after the load.
import { spawn, spawnSync, execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync, existsSync, readdirSync } from 'node:fs';
import http from 'node:http';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = dirname(fileURLToPath(import.meta.url));
const SERVER_CPUS = process.env.SERVER_CPUS || '0-1';
const CLIENT_CPUS = process.env.CLIENT_CPUS || '2-3';
const STEAL_MAX = +(process.env.STEAL_MAX || 5); // percent of CPU time the hypervisor may take during a run
const PORT = 18480; // not 8080: other jobs on a shared host use it
const PATH = ['/root/.cargo/bin', '/root/nr-tools/go/bin', '/root/dotnet', '/usr/local/bin', '/opt/bun/bin', process.env.PATH].join(':');

// One entry per framework: how to build it and how to start it (cwd is apps/<dir>).
const FW = {
  wisp: { name: 'Wisp', build: 'cargo build --release', cmd: './target/release/nr-wisp' },
  aspnet: { name: 'ASP.NET Core', build: 'dotnet publish -c Release -o pub', cmd: './pub/Nr', env: { DOTNET_ROOT: '/root/dotnet' } },
  axum: { name: 'Axum', build: 'cargo build --release', cmd: './target/release/nr-axum' },
  actix: { name: 'Actix Web', build: 'cargo build --release', cmd: './target/release/nr-actix' },
  gin: { name: 'Go Gin', build: 'go mod tidy && go build -o nr-gin .', cmd: './nr-gin' },
  fastify: { name: 'Fastify', build: 'npm install --omit=dev --no-audit --no-fund', cmd: 'node cluster.js', env: { NODE_ENV: 'production' } },
  express: { name: 'Express', build: 'npm install --omit=dev --no-audit --no-fund', cmd: 'node cluster.js', env: { NODE_ENV: 'production' } },
  'hono-bun': { name: 'Hono (Bun)', build: 'bun install', cmd: 'for i in $(seq $(nproc)); do bun server.js & done; wait', env: { NODE_ENV: 'production' } },
  spring: { name: 'Spring Boot', build: 'mvn -q -DskipTests package', cmd: 'java -XX:+UseParallelGC -Xms1g -Xmx1g -jar target/app.jar --server.port=${PORT}' },
  fastapi: { name: 'FastAPI', build: 'python3 -m venv .venv && .venv/bin/pip install -q -r requirements.txt', cmd: `.venv/bin/uvicorn main:app --host 0.0.0.0 --port ${PORT} --workers 2 --http httptools --loop uvloop --no-access-log --log-level error` },
  next: { name: 'Next.js', build: 'npm install --no-audit --no-fund && npx next build', cmd: 'node cluster.js', env: { NODE_ENV: 'production', HOSTNAME: '0.0.0.0' } },
  sveltekit: { name: 'SvelteKit', build: 'npm install --no-audit --no-fund && npx vite build', cmd: 'node cluster.js', env: { NODE_ENV: 'production' } },
};
const ORDER = Object.keys(FW);

// Same URL and headers for every framework.
const ROUTES = {
  plaintext: { path: '/' },
  json: { path: '/json' },
  params: { path: '/params/42?q=hello', headers: ['Cookie: sid=abc123; theme=dark'] },
  list: { path: '/list' },
  'json-big': { path: '/json-big' },
};

const args = process.argv.slice(2);
const cmd = args[0];
const opt = (k, d) => { const i = args.indexOf('--' + k); return i > 0 ? args[i + 1] : d; };
const fws = (opt('fw', '') ? opt('fw').split(',') : ORDER);
const routeNames = opt('routes', '') ? opt('routes').split(',') : Object.keys(ROUTES);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const sh = (c, o = {}) => spawnSync('bash', ['-c', c], { stdio: 'inherit', env: { ...process.env, PATH }, ...o });

function get(path, headers = {}) {
  return new Promise((res, rej) => {
    const r = http.get({ host: '127.0.0.1', port: PORT, path, agent: false, headers, timeout: 5000 }, (m) => {
      const b = [];
      m.on('data', (c) => b.push(c));
      m.on('end', () => res({ status: m.statusCode, type: m.headers['content-type'] || '', body: Buffer.concat(b) }));
    });
    r.on('error', rej);
    r.on('timeout', () => r.destroy(new Error('timeout')));
  });
}

function killStray() {
  spawnSync('bash', ['-c', `fuser -k ${PORT}/tcp >/dev/null 2>&1; true`]);
}

async function start(f) {
  killStray();
  await sleep(500);
  const d = FW[f];
  const env = { ...process.env, PATH, PORT: String(PORT), ...(d.env || {}) };
  const t0 = performance.now();
  const child = spawn('taskset', ['-c', SERVER_CPUS, 'bash', '-c', d.cmd], { cwd: join(ROOT, 'apps', f), env, detached: true, stdio: process.env.NRDEBUG ? 'inherit' : ['pipe', 'ignore', 'ignore'] });
  child.unref(); // stdin stays an open pipe: a server reading a closed stdin (Wisp does) would stop
  for (;;) {
    if (performance.now() - t0 > 90000) throw new Error(`${f}: no 200 within 90 s`);
    try { const r = await get('/'); if (r.status === 200) break; } catch {}
    await sleep(5);
  }
  return { pid: child.pid, coldMs: performance.now() - t0 };
}

function session(pid) {
  try { return execFileSync('ps', ['-o', 'pid=', '--sid', String(pid)], { encoding: 'utf8' }).split(/\s+/).filter(Boolean).map(Number); } catch { return []; }
}
function memKb(pid) {
  let rss = 0, hwm = 0;
  for (const p of session(pid)) {
    try {
      const s = readFileSync(`/proc/${p}/status`, 'utf8');
      rss += +(/VmRSS:\s+(\d+)/.exec(s)?.[1] || 0);
      hwm += +(/VmHWM:\s+(\d+)/.exec(s)?.[1] || 0);
    } catch {}
  }
  return { rssMb: Math.round(rss / 1024), peakMb: Math.round(hwm / 1024) };
}
async function stop(pid) {
  try { process.kill(-pid, 'SIGTERM'); } catch {}
  await sleep(1500);
  try { process.kill(-pid, 'SIGKILL'); } catch {}
  killStray();
  await sleep(1000);
}

function oha(route, secs) {
  const r = ROUTES[route];
  const a = ['-c', CLIENT_CPUS, 'oha', '-z', `${secs}s`, '-c', '64', '--no-tui', '--output-format', 'json'];
  for (const h of r.headers || []) a.push('-H', h);
  a.push(`http://127.0.0.1:${PORT}${r.path}`);
  const out = execFileSync('taskset', a, { encoding: 'utf8', maxBuffer: 1 << 26 });
  const j = JSON.parse(out);
  const codes = j.statusCodeDistribution || {};
  const non200 = Object.entries(codes).filter(([k]) => k !== '200').reduce((s, [, v]) => s + v, 0);
  return { rps: j.summary.requestsPerSec, p50: (j.latencyPercentiles?.p50 ?? 0) * 1000, p99: (j.latencyPercentiles?.p99 ?? 0) * 1000, non200, errors: Object.values(j.errorDistribution || {}).reduce((s, v) => s + v, 0) };
}
// Hypervisor steal over a window: the VPS is shared, so a run's numbers are only as good as its steal.
const cpu = () => {
  const v = readFileSync('/proc/stat', 'utf8').split('\n')[0].trim().split(/\s+/).slice(1).map(Number);
  return [v[7] || 0, v.slice(0, 8).reduce((s, x) => s + x, 0)];
};
const median = (xs) => [...xs].sort((a, b) => a.rps - b.rps)[Math.floor(xs.length / 2)];

async function fetchAll() {
  const o = {};
  for (const [n, r] of Object.entries(ROUTES)) {
    const x = await get(r.path, r.headers ? { Cookie: 'sid=abc123; theme=dark' } : {});
    o[n] = { status: x.status, type: x.type, bytes: x.body.length, sha: createHash('sha1').update(x.body).digest('hex').slice(0, 12), body: x.body.toString() };
  }
  return o;
}

async function bench(f, pass) {
  const runs = +opt('runs', 5), secs = +opt('secs', 10), warm = +opt('warm', 5);
  const file = join(ROOT, 'results', `pass${pass}`, `${f}.json`);
  mkdirSync(dirname(file), { recursive: true });
  const prev = existsSync(file) && args.includes('--routes') ? JSON.parse(readFileSync(file, 'utf8')) : { routes: {} };
  const { pid, coldMs } = await start(f);
  console.log(`[${f}] pass ${pass}: up in ${coldMs.toFixed(0)} ms`);
  const res = { framework: FW[f].name, pass, at: new Date().toISOString(), coldMs: Math.round(coldMs), routes: prev.routes, host: { cpus: SERVER_CPUS, client: CLIENT_CPUS } };
  if (prev.coldMs && args.includes('--routes')) res.coldMs = prev.coldMs;
  try {
    const sample = await fetchAll();
    for (const [n, s] of Object.entries(sample)) if (s.status !== 200) throw new Error(`${n} answered ${s.status}`);
    for (const n of routeNames) {
      oha(n, warm);
      // A run whose hypervisor steal went above STEAL_MAX is thrown away and redone (up to 10
      // tries; then the quietest try is kept and the cell is marked noisy).
      const rs = []; let discarded = 0;
      for (let i = 0; i < runs; i++) {
        let best = null;
        for (let a = 0; a < 10; a++) {
          const c0 = cpu(); const r = oha(n, secs); const c1 = cpu();
          r.steal = 100 * (c1[0] - c0[0]) / (c1[1] - c0[1]);
          if (!best || r.steal < best.steal) best = r;
          if (r.steal <= STEAL_MAX) break;
          discarded++; await sleep(3000);
        }
        rs.push(best); await sleep(500);
      }
      const m = median(rs);
      res.routes[n] = { rps: Math.round(m.rps), p50ms: +m.p50.toFixed(2), p99ms: +m.p99.toFixed(2), non200: rs.reduce((s, x) => s + x.non200, 0), errors: rs.reduce((s, x) => s + x.errors, 0), runs: rs.map((x) => Math.round(x.rps)), stealPct: +median(rs.map((x) => ({ rps: x.steal }))).rps.toFixed(1), discarded, noisy: rs.some((x) => x.steal > STEAL_MAX) };
      console.log(`[${f}] ${n}: ${res.routes[n].rps} req/s p99 ${res.routes[n].p99ms} ms steal ${res.routes[n].stealPct}% discarded ${discarded}${res.routes[n].noisy ? ' NOISY' : ''}  runs ${res.routes[n].runs.join(' ')}`);
    }
    Object.assign(res, memKb(pid));
    console.log(`[${f}] rss ${res.rssMb} MB (peak ${res.peakMb})`);
  } finally {
    await stop(pid);
  }
  writeFileSync(file, JSON.stringify(res, null, 1));
}

async function verify() {
  const all = {};
  for (const f of fws) {
    const { pid } = await start(f);
    try { all[f] = await fetchAll(); } finally { await stop(pid); }
  }
  // Normalise away what a template engine may legitimately differ in: the list page's wrapper.
  const items = (b) => [...b.matchAll(/<li>(.*?)<\/li>/g)].map((m) => m[1].replaceAll('&gt;', '>')); // Svelte leaves a text `>` bare; both are the same text
  const ref = all[fws[0]];
  let bad = 0;
  for (const f of fws) {
    for (const [n, s] of Object.entries(all[f])) {
      let ok = s.status === 200;
      const r = ref[n];
      if (n === 'list') ok &&= JSON.stringify(items(s.body)) === JSON.stringify(items(r.body)) && items(s.body).length === 1000;
      else if (n === 'json-big') ok &&= JSON.stringify(JSON.parse(s.body)) === JSON.stringify(JSON.parse(r.body));
      else ok &&= s.body === r.body;
      const bytesOk = n === 'list' || s.bytes === r.bytes;
      if (!ok || !bytesOk) bad++;
      console.log(`${ok && bytesOk ? 'ok  ' : 'DIFF'} ${f.padEnd(9)} ${n.padEnd(9)} ${String(s.bytes).padStart(6)} B  ${s.type}`);
    }
  }
  mkdirSync(join(ROOT, 'results'), { recursive: true });
  writeFileSync(join(ROOT, 'results', 'verify.json'), JSON.stringify(Object.fromEntries(Object.entries(all).map(([f, o]) => [f, Object.fromEntries(Object.entries(o).map(([n, s]) => [n, { status: s.status, type: s.type, bytes: s.bytes, sha: s.sha }]))])), null, 1));
  console.log(bad ? `${bad} differences` : 'all frameworks agree');
  process.exit(bad ? 1 : 0);
}

if (cmd === 'build') {
  for (const f of fws) {
    console.log(`== build ${f}`);
    const r = sh(FW[f].build, { cwd: join(ROOT, 'apps', f) });
    if (r.status !== 0) console.log(`!! ${f} build failed`);
  }
} else if (cmd === 'verify') await verify();
else if (cmd === 'bench') for (const f of fws) await bench(f, +opt('pass', 1));
else { console.log('usage: node run.mjs build|verify|bench [--pass N] [--fw a,b] [--routes r,..]'); process.exit(2); }
