// Edge bench: Wisp's wasm build against Hono, SvelteKit and Next.js, on the
// same runtime, with oha as the load generator. See README.md for setup.
//   node run.mjs [--dir <bench dir>] [--only wisp-node,hono-cf] [--secs 10] [--conns 64] [--runs 3] [--routes list,json]
import { spawn, execFileSync } from 'node:child_process';
import { join } from 'node:path';

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
const jar = 'sid=abc123; theme=dark';
const routes = ['/', '/list', '/json', '/list1000', '/json-big', '/about', '/params/42?q=hello%20world&x=1'].filter((r) => !arg('routes', '') || arg('routes', '').split(',').some((m) => r.includes(m)));
const hdr = (p) => (p.startsWith('/params') ? { cookie: jar } : {});

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const get = (p) => fetch(`http://127.0.0.1:${PORT}${p}`, { headers: { connection: 'close', ...hdr(p) } }).then((r) => r.text());

async function waitUp(child) {
  for (let i = 0; i < 120; i++) {
    if (child.exitCode !== null) throw new Error('server exited');
    try { await get('/json'); return; } catch { await sleep(500); }
  }
  throw new Error('server did not start');
}

function oha(path, s) {
  const out = execFileSync('oha', ['-z', `${s}s`, '-c', conns, '--no-tui', '--output-format', 'json', ...(path.startsWith('/params') ? ['-H', `cookie: ${jar}`] : []), `http://127.0.0.1:${PORT}${path}`], { maxBuffer: 1 << 26 });
  const j = JSON.parse(out);
  const bad = Object.entries(j.statusCodeDistribution).filter(([c]) => c !== '200').length;
  return { rps: j.summary.requestsPerSec, p99: j.latencyPercentiles.p99 * 1000, bad };
}
const median = (a) => [...a].sort((x, y) => x - y)[a.length >> 1];

const results = {};
for (const [name, [cwd, cmd, args, env]] of Object.entries(servers)) {
  if (only.length && !only.includes(name)) continue;
  const child = spawn(cmd, args, { cwd: join(dir, cwd), env: { ...process.env, PORT: String(PORT), ...env }, stdio: 'ignore' });
  try {
    await waitUp(child);
    for (const p of routes) {
      await get(p);
      oha(p, 3); // warmup
      const rs = Array.from({ length: runs }, () => oha(p, secs));
      results[`${name} ${p}`] = { rps: Math.round(median(rs.map((r) => r.rps))), p99: +median(rs.map((r) => r.p99)).toFixed(2), bad: rs.reduce((a, r) => a + r.bad, 0) };
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
