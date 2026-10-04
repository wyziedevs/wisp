// Wisp against Hono on bare workerd (no wrangler: its dev proxy caps both at
// ~1.7k req/s). See README.md.
//   node workerd.mjs --workerd <workerd binary> --wisp <wisp build dir> --hono <dir with the bundled worker>
//                    [--secs 10] [--conns 64] [--runs 3] [--cold 10] [--only wisp]
// Both run at once; every route's runs alternate between them, so a busy
// machine hurts both the same.
import { spawn, execFileSync } from 'node:child_process';
import { writeFileSync, readFileSync, readdirSync } from 'node:fs';
import { join, resolve } from 'node:path';

const arg = (k, d) => { const i = process.argv.indexOf('--' + k); return i > 0 ? process.argv[i + 1] : d; };
const bin = resolve(arg('workerd', 'workerd'));
const secs = arg('secs', '10'), conns = arg('conns', '64'), runs = Number(arg('runs', '3')), colds = Number(arg('cold', '10'));
const only = arg('only', '');
const apps = Object.entries({ wisp: arg('wisp', '.'), hono: arg('hono', '.') }).filter(([n]) => !only || n === only).map(([name, dir], i) => ({ name, dir: resolve(dir), port: 4300 + i }));
const routes = ['/', '/list', '/json'];

// Every file of the dir is a module: .wasm as wasm, .js and .mjs as ES modules.
// The entry (worker.js, or Hono's bundle app.mjs) goes first.
function config(a) {
  const entry = (f) => (f === 'worker.js' || f === 'app.mjs' ? 0 : 1);
  const mods = readdirSync(a.dir).filter((f) => /\.(m?js|wasm)$/.test(f)).sort((x, y) => entry(x) - entry(y)).map((f) => `(name = "${f}", ${f.endsWith('.wasm') ? 'wasm' : 'esModule'} = embed "${f}")`);
  const path = join(a.dir, 'workerd.capnp');
  writeFileSync(path, `using Workerd = import "/workerd/workerd.capnp";
const config :Workerd.Config = (
  services = [(name = "main", worker = (modules = [${mods.join(', ')}], compatibilityDate = "2025-09-01"))],
  sockets = [(name = "http", address = "127.0.0.1:${a.port}", http = (), service = "main")],
);
`);
  return path;
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const kill = (c) => { if (process.platform === 'win32') try { execFileSync('taskkill', ['/PID', String(c.pid), '/T', '/F'], { stdio: 'ignore' }); } catch {} else c.kill('SIGKILL'); };
const median = (a) => [...a].sort((x, y) => x - y)[a.length >> 1];
const start = (a) => spawn(bin, ['serve', config(a)], { cwd: a.dir, stdio: 'ignore' });

// Time from spawn to the first complete response.
async function cold(a) {
  const t = performance.now();
  const c = start(a);
  for (;;) {
    try { await (await fetch(`http://127.0.0.1:${a.port}/json`, { headers: { connection: 'close' } })).text(); break; } catch { await sleep(2); }
  }
  const ms = performance.now() - t;
  kill(c);
  await sleep(300);
  return ms;
}

// CPU seconds the process has used so far.
function cpu(pid) {
  if (process.platform === 'win32') return Number(execFileSync('powershell', ['-NoProfile', '-Command', `(Get-Process -Id ${pid}).TotalProcessorTime.TotalSeconds`]).toString());
  const f = readFileSync(`/proc/${pid}/stat`, 'utf8').split(') ')[1].split(' ');
  return (Number(f[11]) + Number(f[12])) / 100;
}

// One oha run: req/s, p99 ms, and workerd's CPU microseconds per request.
function oha(a, path, s) {
  const c0 = cpu(a.child.pid);
  const j = JSON.parse(execFileSync('oha', ['-z', `${s}s`, '-c', conns, '--no-tui', '--output-format', 'json', `http://127.0.0.1:${a.port}${path}`], { maxBuffer: 1 << 26 }));
  const used = cpu(a.child.pid) - c0;
  const n = j.statusCodeDistribution['200'] ?? 0;
  const bad = Object.entries(j.statusCodeDistribution).filter(([k]) => k !== '200').length;
  return { rps: j.summary.requestsPerSec, p99: j.latencyPercentiles.p99 * 1000, us: (used * 1e6) / n, bad };
}

for (const a of apps) {
  const cs = [];
  for (let i = 0; i < colds; i++) cs.push(await cold(a));
  console.log(a.name, `cold start ms (median of ${colds})`, median(cs).toFixed(0));
}
for (const a of apps) a.child = start(a);
await sleep(1500);
for (const p of routes) {
  const rs = apps.map(() => []);
  for (const a of apps) { await fetch(`http://127.0.0.1:${a.port}${p}`, { headers: { connection: 'close' } }).then((r) => r.text()); oha(a, p, 3); }
  for (let i = 0; i < runs; i++) apps.forEach((a, k) => rs[k].push(oha(a, p, secs)));
  apps.forEach((a, k) => {
    const m = (f) => +median(rs[k].map((r) => r[f])).toFixed(1);
    console.log(a.name, p, JSON.stringify({ rps: Math.round(m('rps')), p99: m('p99'), cpu_us: m('us'), best_us: +Math.min(...rs[k].map((r) => r.us)).toFixed(1), bad: rs[k].reduce((n, r) => n + r.bad, 0) }));
  });
}
for (const a of apps) kill(a.child);
