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

// Failed requests in one oha run: every reply that is not a 200, counted per
// request (not per distinct status). Also oha's own error distribution.
export function failedCount(j) {
  let n = 0;
  for (const [k, v] of Object.entries(j.statusCodeDistribution || {})) if (k !== '200') n += v;
  // oha 1.16 reports requests still in flight when the -z deadline hits as 'aborted due to deadline': the end of a timed run, not a failure.
  for (const [k, v] of Object.entries(j.errorDistribution || {})) if (k !== 'aborted due to deadline') n += v;
  return n;
}

// Medians over a cell's runs. Any failed request makes the cell invalid: its
// req/s and p99 are null, never a number, and `failed` says how many requests.
export function cellOf(rs, fields = ['rps', 'p99']) {
  const bad = rs.reduce((n, r) => n + r.bad, 0);
  const c = { bad };
  for (const f of fields) c[f] = bad ? null : +median(rs.map((r) => r[f])).toFixed(1);
  if (bad) c.failed = true;
  return c;
}

// A cell from a results file counts only when no request failed.
export const cellValid = (c) => !!c && !c.failed && !c.bad && c.rps != null;

// Why a reply is wrong for route `p`, or null when it is right. The same answers
// whatever the framework: exact text, parsed JSON, HTML with escaped items.
const ref = Array.from({ length: 200 }, (_, k) => ({ id: k + 1, name: `user-${k + 1}`, active: (k + 1) % 3 !== 0, score: ((k + 1) * 37) % 101, tags: ['a', `t${(k + 1) % 7}`] }));
const lis = (t) => (t.match(/<li[ >]/g) || []).length;
export function badReply(p, status, t) {
  if (status !== 200) return `${p} status ${status}`;
  let ok;
  try {
    if (p === '/') ok = t === 'hello';
    else if (p === '/list') ok = lis(t) === 50 && /Item &lt;50(&gt;|>) &amp; co/.test(t);
    else if (p === '/list1000') ok = lis(t) === 1000 && /Item &lt;1000(&gt;|>) &amp; co/.test(t);
    else if (p === '/json') { const j = JSON.parse(t); ok = j.ok === true && j.n === 42; }
    else if (p === '/json-big') ok = JSON.stringify(JSON.parse(t)) === JSON.stringify(ref);
    else if (p === '/about') ok = /Static (&amp;|&) constant\./.test(t);
    else ok = t === 'id=42 q=hello world sid=abc123';
  } catch { ok = false; }
  return ok ? null : `${p} wrong body ${t.slice(0, 80)}`;
}

// Cold-start numbers: failed starts are NaN and dropped. Returns the median of
// the rest and how many there were.
export function coldOf(cs) {
  const ok = cs.filter((x) => x === x);
  return ok.length ? { median: +median(ok).toFixed(1), min: +Math.min(...ok).toFixed(1), n: ok.length } : null;
}

// Wording for a table header: the median over `colds` starts, or over fewer when
// some failed (and were dropped).
export function coldNote(colds, cold) {
  const ns = Object.values(cold || {}).map((c) => c.n ?? colds);
  const lo = Math.min(...ns, colds);
  return lo < colds ? `cold start median of the successful starts (${lo} to ${colds} of ${colds} per framework; failed starts dropped)` : `cold start median of ${colds}`;
}
