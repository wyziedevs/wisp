#!/usr/bin/env node
// Renders results/pass1 + results/pass2 into markdown: per route, both passes side by side, the
// rank in each pass, the gap between passes (flagged above 5%), and the final rank on the mean.
import { readdirSync, readFileSync, existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = dirname(fileURLToPath(import.meta.url));
const ROUTES = ['plaintext', 'json', 'params', 'list', 'json-big'];
const LABEL = { plaintext: '`/` plaintext', json: '`/json`', params: '`/params/42?q=hello` + cookie', list: '`/list` 1000 items', 'json-big': '`/json-big` 200 objects' };
const load = (p) => {
  const d = join(ROOT, 'results', `pass${p}`);
  const o = {};
  if (existsSync(d)) for (const f of readdirSync(d)) if (f.endsWith('.json')) o[f.slice(0, -5)] = JSON.parse(readFileSync(join(d, f), 'utf8'));
  return o;
};
const P = [load(1), load(2)];
const fws = Object.keys(P[0]).filter((f) => P[1][f] || true);
const name = (f) => (P[0][f] || P[1][f]).framework;
const n = (x) => (x == null ? '-' : x.toLocaleString('en-US'));
const mean = (a) => { const v = a.filter((x) => x != null); return v.length ? v.reduce((s, x) => s + x, 0) / v.length : null; };
const diff = (a, b) => (a != null && b != null ? Math.abs(a - b) / ((a + b) / 2) * 100 : null);
const ranks = (get, asc) => {
  const xs = fws.map((f) => [f, get(f)]).filter(([, v]) => v != null).sort((a, b) => (asc ? a[1] - b[1] : b[1] - a[1]));
  return Object.fromEntries(xs.map(([f], i) => [f, i + 1]));
};
const out = [];
const flagged = [];

out.push('## Throughput (req/s, median of 5 x 10 s; higher is better)\n');
const finalRank = {};
for (const r of ROUTES) {
  const g = (p) => (f) => P[p][f]?.routes?.[r]?.rps ?? null;
  const rk = [ranks(g(0)), ranks(g(1))];
  const m = (f) => mean([g(0)(f), g(1)(f)]);
  const fr = ranks(m);
  finalRank[r] = fr;
  out.push(`### ${LABEL[r]}\n`, '| Rank | Framework | Pass 1 | Pass 2 | Mean | Pass gap | Rank p1/p2 | p99 ms p1/p2 |', '|---|---|---|---|---|---|---|---|');
  for (const f of Object.keys(fr).sort((a, b) => fr[a] - fr[b])) {
    const d = diff(g(0)(f), g(1)(f));
    const flag = d != null && d > 5;
    if (flag) flagged.push(`${name(f)} ${r}: ${d.toFixed(1)}%`);
    out.push(`| ${fr[f]} | ${name(f)} | ${n(g(0)(f))} | ${n(g(1)(f))} | ${n(Math.round(m(f)))} | ${d == null ? '-' : d.toFixed(1) + '%'}${flag ? ' **FLAG**' : ''} | ${rk[0][f] ?? '-'}/${rk[1][f] ?? '-'} | ${P[0][f]?.routes?.[r]?.p99ms ?? '-'} / ${P[1][f]?.routes?.[r]?.p99ms ?? '-'} |`);
  }
  out.push('');
}

out.push('## Rank summary (by mean of both passes; 1 is fastest)\n', `| Framework | ${ROUTES.join(' | ')} | Sum |`, `|---|${ROUTES.map(() => '---').join('|')}|---|`);
const sums = (f) => ROUTES.reduce((s, r) => s + (finalRank[r][f] ?? 12), 0);
for (const f of [...fws].sort((a, b) => sums(a) - sums(b))) out.push(`| ${name(f)} | ${ROUTES.map((r) => finalRank[r][f] ?? '-').join(' | ')} | ${sums(f)} |`);

const metric = (title, key, unit) => {
  const g = (p) => (f) => P[p][f]?.[key] ?? null;
  const m = (f) => mean([g(0)(f), g(1)(f)]);
  const fr = ranks(m, true);
  out.push('', `## ${title} (${unit}; lower is better)\n`, '| Rank | Framework | Pass 1 | Pass 2 | Mean | Pass gap |', '|---|---|---|---|---|---|');
  for (const f of Object.keys(fr).sort((a, b) => fr[a] - fr[b])) {
    const d = diff(g(0)(f), g(1)(f));
    out.push(`| ${fr[f]} | ${name(f)} | ${n(g(0)(f))} | ${n(g(1)(f))} | ${n(Math.round(m(f)))} | ${d == null ? '-' : d.toFixed(1) + '%'}${d != null && d > 5 ? ' (differs; see notes)' : ''} |`);
  }
};
metric('Cold start, spawn to first 200 on `/`', 'coldMs', 'ms');
metric('RSS after load (all processes of the server)', 'rssMb', 'MB');

out.push('', '## Cells where the two passes differ by more than 5%\n', flagged.length ? flagged.map((x) => `- ${x}`).join('\n') : 'none');
console.log(out.join('\n'));
