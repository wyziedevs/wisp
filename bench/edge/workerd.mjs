// Wisp against Hono on bare workerd (no wrangler: its dev proxy caps both at
// ~1.7k req/s). See README.md.
//   node workerd.mjs --workerd <workerd binary> --wisp <wisp build dir> --hono <dir with the bundled worker>
//                    [--secs 10] [--conns 64] [--runs 3] [--cold 10] [--only wisp] [--routes list,json]
// Both run at once; every route's runs alternate between them, so a busy
// machine hurts both the same.
import { spawn, execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { sleep, hdr, oha as ohaRun, workerdConfig, failedCount, cellOf, badReply, coldOf } from './util.mjs';

const arg = (k, d) => { const i = process.argv.indexOf('--' + k); return i > 0 ? process.argv[i + 1] : d; };
const bin = resolve(arg('workerd', 'workerd'));
const secs = arg('secs', '10'), conns = arg('conns', '64'), runs = Number(arg('runs', '3')), colds = Number(arg('cold', '10'));
const only = arg('only', '');
const apps = Object.entries({ wisp: arg('wisp', '.'), hono: arg('hono', '.') }).filter(([n]) => !only || n === only).map(([name, dir], i) => ({ name, dir: resolve(dir), port: 4300 + i }));
const routes = ['/', '/list', '/json', '/list1000', '/json-big', '/about', '/params/42?q=hello%20world&x=1'].filter((r) => !arg('routes', '') || arg('routes', '').split(',').some((m) => (m === '/' ? r === '/' : r.includes(m))));

const config = (a) => workerdConfig(a.dir, a.port, { entries: ['worker.js', 'app.mjs'] });
const kill = (c) => { if (process.platform === 'win32') try { execFileSync('taskkill', ['/PID', String(c.pid), '/T', '/F'], { stdio: 'ignore' }); } catch {} else c.kill('SIGKILL'); };
const start = (a) => spawn(bin, ['serve', config(a)], { cwd: a.dir, stdio: 'ignore' });

const get = (a, p) => fetch(`http://127.0.0.1:${a.port}${p}`, { headers: { connection: 'close', ...hdr(p) } });
// Every route must answer 200 with the right body before anything is timed.
async function verify(a) {
  const bad = [];
  for (const p of routes) { const r = await get(a, p); const why = badReply(p, r.status, await r.text()); if (why) bad.push(why); }
  return bad;
}

// Time from spawn to the first correct response of a measured route; NaN when the start failed.
async function cold(a) {
  const t = performance.now();
  const c = start(a);
  for (;;) {
    if (c.exitCode !== null || performance.now() - t > 60000) { kill(c); return NaN; }
    let r;
    try { r = await get(a, routes[0]); r = badReply(routes[0], r.status, await r.text()); } catch { await sleep(2); continue; }
    if (r) { kill(c); return NaN; }
    break;
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
  const j = ohaRun(`http://127.0.0.1:${a.port}${path}`, path, s, conns);
  const used = cpu(a.child.pid) - c0;
  const n = j.statusCodeDistribution['200'] ?? 0;
  return { rps: j.summary.requestsPerSec, p99: j.latencyPercentiles.p99 * 1000, us: (used * 1e6) / n, bad: failedCount(j) };
}

for (const a of apps) {
  const cs = [];
  for (let i = 0; i < colds; i++) cs.push(await cold(a));
  const c = coldOf(cs);
  console.log(a.name, c ? `cold start ms (median of ${c.n} of ${colds} starts)` : 'cold start FAILED', c ? c.median.toFixed(0) : '');
}
for (const a of apps) a.child = start(a);
await sleep(1500);
const live = [];
for (const a of apps) {
  const bad = await verify(a).catch((e) => [String(e)]);
  if (bad.length) console.log(a.name, 'WRONG OUTPUT, not timed', bad.join(' | '));
  else live.push(a);
}
for (const p of routes) {
  const rs = live.map(() => []);
  for (const a of live) { await get(a, p).then((r) => r.text()); oha(a, p, 3); }
  for (let i = 0; i < runs; i++) live.forEach((a, k) => rs[k].push(oha(a, p, secs)));
  live.forEach((a, k) => {
    const c = cellOf(rs[k], ['rps', 'p99', 'us']);
    console.log(a.name, p, JSON.stringify(c.failed ? { failed: true, bad: c.bad } : { rps: Math.round(c.rps), p99: c.p99, cpu_us: c.us, best_us: +Math.min(...rs[k].map((r) => r.us)).toFixed(1), bad: 0 }));
  });
}
for (const a of apps) kill(a.child);
