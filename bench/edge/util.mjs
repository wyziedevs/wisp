// Shared by the edge and rank benches.
import { execFileSync } from 'node:child_process';
import { writeFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
export const median = (a) => [...a].sort((x, y) => x - y)[a.length >> 1];

export const jar = 'sid=abc123; theme=dark';
export const hdr = (p) => (p.startsWith('/params') ? { cookie: jar } : {});

// One oha run against `url`; `pre` is a command prefix (e.g. taskset). Returns oha's parsed JSON.
export function oha(url, path, secs, conns, pre = []) {
  const [cmd, ...head] = [...pre, 'oha'];
  const h = Object.entries(hdr(path)).flatMap(([k, v]) => ['-H', `${k}: ${v}`]);
  return JSON.parse(execFileSync(cmd, [...head, '-z', `${secs}s`, '-c', conns, '--no-tui', '--output-format', 'json', ...h, url], { maxBuffer: 1 << 26 }));
}

// Writes dir/workerd.capnp and returns its path. Every .js/.mjs file is an ES
// module, .wasm is wasm; files named in `entries` go first. Options: entries,
// deep (recurse into subdirs), date, flags, self (ASSETS 404 worker + WORKER_SELF_REFERENCE).
export function workerdConfig(dir, port, { entries = ['worker.js'], deep = false, date = '2025-09-01', flags = [], self = false } = {}) {
  const files = [];
  const walk = (d) => {
    for (const e of readdirSync(join(dir, d), { withFileTypes: true })) {
      const rel = d === '.' ? e.name : `${d}/${e.name}`;
      if (e.isDirectory()) { if (deep) walk(rel); } else if (/\.(m?js|wasm)$/.test(e.name)) files.push(rel);
    }
  };
  walk('.');
  const rank = (f) => { const i = entries.indexOf(f); return i < 0 ? entries.length : i; };
  files.sort((x, y) => rank(x) - rank(y));
  const mods = files.map((f) => `(name = "${f}", ${f.endsWith('.wasm') ? 'wasm' : 'esModule'} = embed "${f}")`);
  const fl = flags.length ? `, compatibilityFlags = [${flags.map((f) => `"${f}"`).join(', ')}]` : '';
  const bind = self ? `,\n      bindings = [(name = "ASSETS", service = "assets"), (name = "WORKER_SELF_REFERENCE", service = "main")]` : '';
  const assets = self ? `\n    (name = "assets", worker = (serviceWorkerScript = "addEventListener('fetch', (e) => e.respondWith(new Response('not found', { status: 404 })))", compatibilityDate = "2025-09-01")),` : '';
  const path = join(dir, 'workerd.capnp');
  writeFileSync(path, `using Workerd = import "/workerd/workerd.capnp";
const config :Workerd.Config = (
  services = [
    (name = "main", worker = (modules = [${mods.join(', ')}], compatibilityDate = "${date}"${fl}${bind})),${assets}
  ],
  sockets = [(name = "http", address = "127.0.0.1:${port}", http = (), service = "main")],
);
`);
  return path;
}
