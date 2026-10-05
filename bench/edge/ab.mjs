// Wisp against Hono on Node, Bun and Deno, alternating: every route's runs go
// one server after the other, so a busy machine hurts all alike. oha is the
// load generator; cold start is process spawn to the first complete response.
//   node ab.mjs --group node|bun|deno [--dir C:/wb] [--bin <runtime binary>] [--secs 10] [--conns 64] [--runs 5] [--cold 9] [--routes json-big,params]
// Directories under --dir: `cl-node`, `cl-bun`, `cl-deno` (wisp build --target
// node|bun|deno) and `hono` (apps/hono, plus `deno.ts` and `bun.ts`, one line
// each: `import { app } from './app.mjs'; Deno.serve({ port: Number(Deno.env.get('PORT')) }, app.fetch)`).
import { spawn, execFileSync } from 'node:child_process';
import { join } from 'node:path';
import { sleep, hdr, oha as ohaRun, failedCount, cellOf, badReply, coldOf } from './util.mjs';

const arg = (k, d) => { const i = process.argv.indexOf('--' + k); return i > 0 ? process.argv[i + 1] : d; };
const dir = arg('dir', process.env.BENCH_DIR || 'C:/wb');
const group = arg('group', 'node');
const bin = arg('bin', group);
const secs = arg('secs', '10'), conns = arg('conns', '64'), runs = Number(arg('runs', '5')), colds = Number(arg('cold', '9'));
const routes = ['/', '/list1000', '/json-big', '/params/42?q=hello%20world&x=1'].filter((r) => !arg('routes', '') || arg('routes', '').split(',').some((m) => r.includes(m)));

// name -> [cwd, command, args, env]
const sets = {
  node: {
    'wisp raw': ['cl-node', process.execPath, ['server.mjs']],
    'wisp node:http': ['cl-node', process.execPath, ['server.mjs'], { WISP_NODE_HTTP: '1' }],
    hono: ['hono', process.execPath, ['node.mjs']],
  },
  bun: {
    'wisp raw': ['cl-bun', bin, ['server.mjs']],
    'wisp Bun.serve': ['cl-bun', bin, ['server.mjs'], { WISP_NODE_HTTP: '1' }],
    hono: ['hono', bin, ['bun.ts']],
  },
  deno: {
    'wisp raw': ['cl-deno', bin, ['run', '-A', 'main.ts']],
    'wisp Deno.serve': ['cl-deno', bin, ['run', '-A', 'main.ts'], { WISP_NODE_HTTP: '1' }],
    hono: ['hono', bin, ['run', '-A', 'deno.ts']],
  },
}[group];
const apps = Object.entries(sets).map(([name, [cwd, cmd, args, env]], i) => ({ name, cwd: join(dir, cwd), cmd, args, env: { ...env, PORT: String(4500 + i) }, port: 4500 + i }));

const kill = (c) => { if (process.platform === 'win32') try { execFileSync('taskkill', ['/PID', String(c.pid), '/T', '/F'], { stdio: 'ignore' }); } catch {} else c.kill('SIGKILL'); };
const start = (a) => spawn(a.cmd, a.args, { cwd: a.cwd, env: { ...process.env, ...a.env }, stdio: 'ignore' });
const url = (a, p) => `http://127.0.0.1:${a.port}${p}`;
const get = (a, p) => fetch(url(a, p), { headers: { connection: 'close', ...hdr(p) } });
// Every route must answer 200 with the right body before anything is timed.
async function verify(a) {
  const bad = [];
  for (const p of routes) { const r = await get(a, p); const why = badReply(p, r.status, await r.text()); if (why) bad.push(why); }
  return bad;
}

async function cold(a) {
  const t = performance.now();
  const c = start(a);
  for (;;) {
    if (c.exitCode !== null || performance.now() - t > 60000) { kill(c); return NaN; }
    let r;
    try { r = await get(a, routes[0]); r = badReply(routes[0], r.status, await r.text()); } catch { await sleep(2); continue; }
    if (r) { kill(c); return NaN; } // a complete reply that is not the right one: failed start
    break;
  }
  const ms = performance.now() - t;
  kill(c);
  await sleep(500);
  return ms;
}

function oha(a, path, s) {
  const j = ohaRun(url(a, path), path, s, conns);
  return { rps: j.summary.requestsPerSec, p99: j.latencyPercentiles.p99 * 1000, bad: failedCount(j) };
}

try {
  for (const a of apps) {
    const cs = [];
    for (let i = 0; i < colds; i++) cs.push(await cold(a));
    const c = coldOf(cs);
    console.log(group, a.name, c ? `cold start ms (median of ${c.n} of ${colds} starts)` : 'cold start FAILED', c ? c.median.toFixed(0) : '');
  }
  for (const a of apps) a.child = start(a);
  await sleep(2500);
  const live = [];
  for (const a of apps) {
    const bad = await verify(a).catch((e) => [String(e)]);
    if (bad.length) console.log(group, a.name, 'WRONG OUTPUT, not timed', bad.join(' | '));
    else live.push(a);
  }
  for (const p of routes) {
    const rs = live.map(() => []);
    for (const a of live) { await get(a, p).then((r) => r.text()); oha(a, p, 3); }
    for (let i = 0; i < runs; i++) live.forEach((a, k) => rs[k].push(oha(a, p, secs)));
    live.forEach((a, k) => {
      const c = cellOf(rs[k]);
      console.log(group, a.name, p, JSON.stringify(c.failed ? { failed: true, bad: c.bad } : { rps: Math.round(c.rps), p99: c.p99, bad: 0 }));
    });
  }
} finally {
  for (const a of apps) if (a.child) kill(a.child);
}
