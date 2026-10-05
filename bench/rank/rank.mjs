// Ranking bench: Wisp against the popular frameworks of each host, on a quiet
// Linux box. Same routes and load tool as ../edge (oha, c=64, median of 5 x
// 10 s), plus cold start and memory. Servers run on cores 0-1, oha on 2-3.
//   node rank.mjs --host workerd|node|bun|deno [--dir <out dir>] [--only a,b] [--secs 10] [--runs 5] [--cold 15] [--conns 64]
// All of a host's servers run at once and every route's runs alternate between
// them, so noise hurts all alike. Results go to results/<host>.json; `node
// report.mjs` renders the tables.
import { spawn } from 'node:child_process';
import { writeFileSync, readFileSync, readdirSync, existsSync, mkdirSync } from 'node:fs';
import { join, resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { sleep, hdr, oha as ohaRun, workerdConfig, failedCount, cellOf, badReply, coldOf } from '../edge/util.mjs';

const arg = (k, d) => { const i = process.argv.indexOf('--' + k); return i > 0 ? process.argv[i + 1] : d; };
const here = dirname(fileURLToPath(import.meta.url));
const host = arg('host', 'node');
const out = resolve(arg('dir', join(here, 'out')));
const secs = arg('secs', '10'), conns = arg('conns', '64'), runs = Number(arg('runs', '5')), colds = Number(arg('cold', '15'));
const only = arg('only', '').split(',').filter(Boolean);
const SRV_CPUS = arg('srv-cpus', '0,1'), LOAD_CPUS = arg('load-cpus', '2,3');
const only_routes = arg('routes', '');
const routes = ['/', '/list1000', '/json-big', '/params/42?q=hello%20world&x=1'].filter((r) => !only_routes || only_routes.split(',').some((m) => (m === '/' ? r === '/' : r.includes(m))));
const bin = (n) => process.env[n.toUpperCase() + '_BIN'] || n;

// name -> { wisp?, dir (under out), cmd, args, env, workerd? }
const node = process.execPath;
const sets = {
  workerd: {
    wisp: { wisp: 1, workerd: 1, dir: 'wisp-cf' },
    hono: { workerd: 1, dir: 'hono-cf' },
    itty: { workerd: 1, dir: 'itty-cf' },
    sveltekit: { workerd: 1, dir: 'sveltekit-cf' },
    astro: { workerd: 1, dir: 'astro-cf' },
    'react-router': { workerd: 1, dir: 'rr-cf' },
    next: { workerd: 1, dir: 'next-cf' },
  },
  node: {
    'wisp raw': { wisp: 1, dir: 'wisp-node', cmd: node, args: ['server.mjs'] },
    'wisp node:http': { wisp: 1, dir: 'wisp-node', cmd: node, args: ['server.mjs'], env: { WISP_NODE_HTTP: '1' } },
    hono: { dir: 'micro', cmd: node, args: ['node-hono.mjs'] },
    fastify: { dir: 'micro', cmd: node, args: ['fastify.mjs'] },
    express: { dir: 'micro', cmd: node, args: ['express.mjs'] },
    sveltekit: { dir: 'sveltekit-node', cmd: node, args: ['index.js'] },
    next: { dir: 'next-node', cmd: node, args: ['server.js'], env: { HOSTNAME: '127.0.0.1' } },
  },
  bun: {
    'wisp raw': { wisp: 1, dir: 'wisp-bun', cmd: bin('bun'), args: ['server.mjs'] },
    'wisp Bun.serve': { wisp: 1, dir: 'wisp-bun', cmd: bin('bun'), args: ['server.mjs'], env: { WISP_NODE_HTTP: '1' } },
    hono: { dir: 'micro', cmd: bin('bun'), args: ['hono-bun.ts'] },
    elysia: { dir: 'micro', cmd: bin('bun'), args: ['elysia.ts'] },
  },
  deno: {
    'wisp raw': { wisp: 1, dir: 'wisp-deno', cmd: bin('deno'), args: ['run', '-A', 'main.ts'] },
    'wisp Deno.serve': { wisp: 1, dir: 'wisp-deno', cmd: bin('deno'), args: ['run', '-A', 'main.ts'], env: { WISP_NODE_HTTP: '1' } },
    hono: { dir: 'micro', cmd: bin('deno'), args: ['run', '-A', 'hono-deno.ts'] },
    oak: { dir: 'oak', cmd: bin('deno'), args: ['run', '-A', 'oak.ts'] },
    fresh: { dir: 'fresh', cmd: bin('deno'), args: ['serve', '-A', '--host', '127.0.0.1', '--port', '{port}', '_fresh/server.js'] },
  },
}[host];

// workerd: every .js/.mjs file of the app dir is an ES module, .wasm is wasm;
// compat date and flags come from meta.json (written by build.sh). ASSETS is a
// worker that answers 404; WORKER_SELF_REFERENCE points at the worker itself.
const workerd = (a) => {
  const meta = existsSync(join(a.cwd, 'meta.json')) ? JSON.parse(readFileSync(join(a.cwd, 'meta.json'), 'utf8')) : {};
  return workerdConfig(a.cwd, a.port, { entries: [meta.entry || 'worker.js'], deep: true, date: meta.date, flags: meta.flags, self: true });
};

const apps = Object.entries(sets).filter(([n]) => !only.length || only.includes(n)).map(([name, s], i) => {
  const a = { name, wisp: !!s.wisp, cwd: join(out, s.dir), port: 4600 + i, env: { ...s.env, PORT: String(4600 + i) } };
  if (!existsSync(a.cwd)) return a;
  a.cmd = s.workerd ? bin('workerd') : s.cmd;
  a.args = s.workerd ? ['serve', workerd(a)] : s.args.map((x) => x.replace('{port}', a.port));
  return a;
});

const start = (a) => spawn('taskset', ['-c', SRV_CPUS, a.cmd, ...a.args], { cwd: a.cwd, env: { ...process.env, NODE_ENV: 'production', ...a.env }, stdio: 'ignore', detached: true });
// Kill the whole process group (Next, Deno and friends leave children).
const kill = (c) => { try { process.kill(-c.pid, 'SIGKILL'); } catch {} };
const url = (a, p) => `http://127.0.0.1:${a.port}${p}`;
const get = (a, p) => fetch(url(a, p), { headers: { connection: 'close', ...hdr(p) } });

// Resident memory of the server and all its children, MB.
function rss(pid) {
  const ppid = new Map(), kb = new Map();
  for (const d of readdirSync('/proc').filter((x) => /^\d+$/.test(x))) {
    try {
      const st = readFileSync(`/proc/${d}/stat`, 'utf8');
      ppid.set(+d, +st.slice(st.lastIndexOf(')') + 2).split(' ')[1]);
      kb.set(+d, +/VmRSS:\s+(\d+)/.exec(readFileSync(`/proc/${d}/status`, 'utf8'))?.[1] || 0);
    } catch {}
  }
  let sum = 0;
  const walk = (p) => { sum += kb.get(p) || 0; for (const [c, pp] of ppid) if (pp === p) walk(c); };
  walk(pid);
  return Math.round(sum / 1024);
}

// Same answers whatever the framework (badReply in ../edge/util.mjs).
async function check(a) {
  const bad = [];
  for (const p of routes) { const r = await get(a, p); const why = badReply(p, r.status, await r.text()); if (why) bad.push(why); }
  return bad;
}

async function cold(a) {
  const t = performance.now();
  const c = start(a);
  for (;;) {
    if (c.exitCode !== null) return NaN;
    if (performance.now() - t > 60000) { kill(c); return NaN; }
    // Cold start is a measured route answering 200 with the right body.
    let why;
    try { const r = await get(a, routes[0]); why = badReply(routes[0], r.status, await r.text()); } catch { await sleep(2); continue; }
    if (why) { kill(c); return NaN; }
    break;
  }
  const ms = performance.now() - t;
  kill(c);
  await sleep(400);
  return ms;
}

function oha(a, path, s) {
  const j = ohaRun(url(a, path), path, s, conns, ['taskset', '-c', LOAD_CPUS]);
  return { rps: j.summary.requestsPerSec, p99: j.latencyPercentiles.p99 * 1000, bad: failedCount(j) };
}

const res = { host, when: new Date().toISOString(), secs, runs, conns, colds, cells: {}, cold: {}, rss: {}, failed: {} };
mkdirSync(join(here, 'results'), { recursive: true });
const save = () => writeFileSync(join(here, 'results', `${host}.json`), JSON.stringify(res, null, 1));
const live = [];
try {
  // Cold starts alternate between the servers too, a round at a time.
  const there = [];
  for (const a of apps) {
    if (existsSync(a.cwd)) there.push(a);
    else { res.failed[a.name] = 'missing ' + a.cwd; console.log(host, a.name, 'MISSING'); }
  }
  const cs = new Map(there.map((a) => [a, []]));
  for (let i = 0; i < colds; i++) for (const a of there) cs.get(a).push(await cold(a));
  for (const a of there) {
    const c = coldOf(cs.get(a));
    if (!c) { res.failed[a.name] = 'does not start'; console.log(host, a.name, 'DOES NOT START'); continue; }
    res.cold[a.name] = c;
    console.log(host, a.name, 'cold ms', res.cold[a.name].median);
    a.child = start(a);
    await sleep(3000);
    const bad = await check(a).catch((e) => [String(e)]);
    if (bad.length) { res.failed[a.name] = bad.join(' | '); console.log(host, a.name, 'WRONG OUTPUT', bad); kill(a.child); continue; }
    live.push(a);
  }
  save();
  for (const a of live) res.rss[a.name] = { idle: rss(a.child.pid) };
  for (const p of routes) {
    const rs = live.map(() => []);
    for (const a of live) { await get(a, p).then((r) => r.text()); oha(a, p, 3); }
    for (let i = 0; i < runs; i++) live.forEach((a, k) => rs[k].push(oha(a, p, secs)));
    live.forEach((a, k) => {
      // Any failed request: no req/s (null), `failed: true`; report.mjs prints Failed and does not rank it.
      const c = cellOf(rs[k]);
      res.cells[`${a.name} ${p}`] = { rps: c.rps == null ? null : Math.round(c.rps), p99: c.p99, runs: rs[k].map((r) => Math.round(r.rps)), bad: c.bad, ...(c.failed && { failed: true }) };
      console.log(host, a.name, p, JSON.stringify(res.cells[`${a.name} ${p}`]));
    });
    save();
  }
  for (const a of live) res.rss[a.name].load = rss(a.child.pid);
  save();
} finally {
  for (const a of apps) if (a.child) kill(a.child);
}
