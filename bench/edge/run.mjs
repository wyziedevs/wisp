// Edge bench: Wisp's wasm build against Hono, SvelteKit and Next.js, on the
// same runtime, with oha as the load generator. See README.md for setup.
//   node run.mjs [--dir <bench dir>] [--only wisp-node,hono-cf] [--secs 10] [--conns 64] [--runs 3] [--routes list,json]
import { spawn, execFileSync } from 'node:child_process';
import { join } from 'node:path';
import { sleep, hdr, oha as ohaRun, failedCount, cellOf, badReply } from './util.mjs';

const arg = (k, d) => { const i = process.argv.indexOf('--' + k); return i > 0 ? process.argv[i + 1] : d; };
const dir = arg('dir', process.env.BENCH_DIR || 'C:/wb');
const only = arg('only', '').split(',').filter(Boolean);
const secs = arg('secs', '10'), conns = arg('conns', '64'), runs = Number(arg('runs', '3'));
const wrangler = join(dir, 'hono/node_modules/wrangler/bin/wrangler.js');
const node = process.execPath;
const PORT = 4200;

// name -> [cwd, command, args, env]
const servers = {
  'wisp-node': ['wisp-node', node, ['server.mjs']],
  'wisp-node-http': ['wisp-node', node, ['server.mjs'], { WISP_NODE_HTTP: '1' }],
  'hono-node': ['hono', node, ['node.mjs']],
  'hello-node': ['hello', node, ['node.mjs']],
  'sveltekit-node': ['sk', node, ['build/index.js']],
  'next-node': ['next', node, ['.next/standalone/server.js'], { HOSTNAME: '127.0.0.1' }],
  'wisp-cf': ['wisp-cf', node, [wrangler, 'dev', '--local', '--log-level', 'error', '--port', String(PORT)]],
  'hono-cf': ['hono', node, [wrangler, 'dev', '--local', '--log-level', 'error', '--port', String(PORT)]],
};
const routes = ['/', '/list', '/json', '/list1000', '/json-big', '/about', '/params/42?q=hello%20world&x=1'].filter((r) => !arg('routes', '') || arg('routes', '').split(',').some((m) => r.includes(m)));

const get = (p) => fetch(`http://127.0.0.1:${PORT}${p}`, { headers: { connection: 'close', ...hdr(p) } });
// Every route must answer 200 with the right body before anything is timed.
async function verify() {
  const bad = [];
  for (const p of routes) { const r = await get(p); const why = badReply(p, r.status, await r.text()); if (why) bad.push(why); }
  if (bad.length) throw new Error('wrong output, not timed: ' + bad.join(' | '));
}

async function waitUp(child) {
  for (let i = 0; i < 120; i++) {
    if (child.exitCode !== null) throw new Error('server exited');
    try { await (await get('/json')).text(); return; } catch { await sleep(500); }
  }
  throw new Error('server did not start');
}

function oha(path, s) {
  const j = ohaRun(`http://127.0.0.1:${PORT}${path}`, path, s, conns);
  return { rps: j.summary.requestsPerSec, p99: j.latencyPercentiles.p99 * 1000, bad: failedCount(j, conns) };
}

const results = {};
for (const [name, [cwd, cmd, args, env]] of Object.entries(servers)) {
  if (only.length && !only.includes(name)) continue;
  const child = spawn(cmd, args, { cwd: join(dir, cwd), env: { ...process.env, PORT: String(PORT), ...env }, stdio: 'ignore' });
  try {
    await waitUp(child);
    await verify();
    for (const p of routes) {
      await get(p).then((r) => r.text());
      oha(p, 3); // warmup
      const rs = Array.from({ length: runs }, () => oha(p, secs));
      const c = cellOf(rs);
      results[`${name} ${p}`] = c.failed ? { failed: true, bad: c.bad } : { rps: Math.round(c.rps), p99: c.p99, bad: 0 };
      console.log(name, p, JSON.stringify(results[`${name} ${p}`]));
    }
  } catch (e) {
    console.log(name, 'FAILED', e.message);
  } finally {
    if (process.platform === 'win32') try { execFileSync('taskkill', ['/PID', String(child.pid), '/T', '/F'], { stdio: 'ignore' }); } catch {}
    else child.kill('SIGKILL');
    await sleep(1500);
  }
}
console.log(JSON.stringify(results));
