#!/usr/bin/env node
// Renders results/pass{1..5}/*.json into markdown. Per route: every pass side by side (~ marks a
// pass whose host steal stayed above the gate), the final value, and the rank. Final value: the
// median of the clean passes; a cell with fewer than two clean passes is not ranked. A cell "agrees" when two
// clean passes are within 5% of each other; otherwise it is flagged and its rank is provisional.
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
const P = [1, 2, 3, 4, 5].map(load).filter((x) => Object.keys(x).length);
const fws = [...new Set(P.flatMap((x) => Object.keys(x)))];
const name = (f) => P.map((x) => x[f]?.framework).find(Boolean);
const n = (x) => (x == null ? '-' : Math.round(x).toLocaleString('en-US'));
const median = (a) => { const s = [...a].sort((x, y) => x - y); return s.length % 2 ? s[(s.length - 1) / 2] : (s[s.length / 2 - 1] + s[s.length / 2]) / 2; };
const gap = (a, b) => Math.abs(a - b) / ((a + b) / 2) * 100;

// One cell: all passes, the clean ones, the final number, whether two clean passes agree.
function cell(f, r) {
  const v = P.map((x) => x[f]?.routes?.[r]).map((c) => (c ? { rps: c.rps, noisy: !!c.noisy, p99: c.p99ms, steal: c.stealPct } : null));
  const have = v.filter(Boolean);
  const clean = have.filter((c) => !c.noisy);
  const use = clean.length >= 2 ? clean : [];
  let agree = false;
  for (let i = 0; i < clean.length; i++) for (let j = i + 1; j < clean.length; j++) if (gap(clean[i].rps, clean[j].rps) <= 5) agree = true;
  return { v, final: use.length ? median(use.map((c) => c.rps)) : null, agree, nclean: clean.length };
}
const ranks = (get, asc) => {
  const xs = fws.map((f) => [f, get(f)]).filter(([, x]) => x != null).sort((a, b) => (asc ? a[1] - b[1] : b[1] - a[1]));
  return Object.fromEntries(xs.map(([f], i) => [f, i + 1]));
};

const out = [];
const flagged = [];
const finalRank = {};
const passCols = P.map((_, i) => `Pass ${i + 1}`).join(' | ');
out.push(`## Throughput (req/s, median of 5 x 10 s per pass; higher is better)\n`, '`~` = host steal stayed above the gate in that pass. Final = median of the clean passes (needs at least two, else the cell is unranked). Status: ok = two clean passes within 5%; **no agree** = they never did (the shared host, see README).\n');
for (const r of ROUTES) {
  const cells = Object.fromEntries(fws.map((f) => [f, cell(f, r)]));
  const fr = ranks((f) => cells[f].final);
  finalRank[r] = fr;
  out.push(`### ${LABEL[r]}\n`, `| Rank | Framework | ${passCols} | Final | Status |`, `|---|---|${P.map(() => '---').join('|')}|---|---|`);
  for (const f of Object.keys(fr).sort((a, b) => fr[a] - fr[b])) {
    const c = cells[f];
    if (!c.agree) flagged.push(`${name(f)} ${r}`);
    out.push(`| ${fr[f]} | ${name(f)} | ${c.v.map((x) => (x ? n(x.rps) + (x.noisy ? '~' : '') : '-')).join(' | ')} | ${n(c.final)} | ${c.agree ? 'ok' : '**no agree**'} |`);
  }
  for (const f of fws) if (cells[f].v.some(Boolean) && fr[f] == null) {
    flagged.push(`${name(f)} ${r}`);
    out.push(`| - | ${name(f)} | ${cells[f].v.map((x) => (x ? n(x.rps) + (x.noisy ? '~' : '') : '-')).join(' | ')} | - | unranked: too few clean passes |`);
  }
  out.push('');
}

out.push('## Rank summary (by final value; 1 is fastest)\n', `| Framework | ${ROUTES.join(' | ')} | Sum (ranked cells) | Ranked cells |`, `|---|${ROUTES.map(() => '---').join('|')}|---|---|`);
// Unranked cells (fewer than two clean passes) add nothing to the sum; the column counts the ranked cells.
const sums = (f) => ROUTES.reduce((s, r) => s + (finalRank[r][f] ?? 0), 0);
const nr = (f) => ROUTES.filter((r) => finalRank[r][f] != null).length;
const order = (a, b) => nr(b) - nr(a) || sums(a) - sums(b);
for (const f of [...fws].sort(order)) out.push(`| ${name(f)} | ${ROUTES.map((r) => finalRank[r][f] ?? '-').join(' | ')} | ${sums(f)} | ${nr(f)} |`);

out.push('', '## Per pass ranks (what each pass said on its own)\n', `| Framework | ${P.map((_, i) => `Pass ${i + 1}: ${ROUTES.join(' ')}`).join(' | ')} |`, `|---|${P.map(() => '---').join('|')}|`);
const pr = P.map((x) => Object.fromEntries(ROUTES.map((r) => [r, ranks((f) => x[f]?.routes?.[r]?.rps ?? null)])));
for (const f of [...fws].sort(order)) out.push(`| ${name(f)} | ${P.map((_, i) => ROUTES.map((r) => pr[i][r][f] ?? '-').join(' ')).join(' | ')} |`);

// CPU per request: user+system time of the server's processes per request. Hypervisor steal does
// not count as the server's CPU, so this ranks the frameworks by work done, steal or not.
const cpuOf = (f, r) => { const v = P.map((x) => x[f]?.routes?.[r]?.cpuUs).filter((x) => x != null); return v.length ? median(v) : null; };
if (fws.some((f) => ROUTES.some((r) => cpuOf(f, r) != null))) {
  const cr = Object.fromEntries(ROUTES.map((r) => [r, ranks((f) => cpuOf(f, r), true)]));
  const csum = (f) => ROUTES.reduce((s, r) => s + (cr[r][f] ?? 12), 0);
  out.push('', '## CPU per request (server CPU microseconds, user+system, both cores; lower is better; rank in brackets)\n', `| Framework | ${ROUTES.join(' | ')} | Sum of ranks |`, `|---|${ROUTES.map(() => '---').join('|')}|---|`);
  for (const f of [...fws].sort((a, b) => csum(a) - csum(b))) out.push(`| ${name(f)} | ${ROUTES.map((r) => `${cpuOf(f, r)?.toFixed(1) ?? '-'} (${cr[r][f] ?? '-'})`).join(' | ')} | ${csum(f)} |`);
}

const metric = (title, key, unit) => {
  const g = (f) => { const v = P.map((x) => x[f]?.[key]).filter((x) => x != null); return v.length ? median(v) : null; };
  const fr = ranks(g, true);
  out.push('', `## ${title} (${unit}; lower is better; median of the passes)\n`, `| Rank | Framework | ${passCols} | Median |`, `|---|---|${P.map(() => '---').join('|')}|---|`);
  for (const f of Object.keys(fr).sort((a, b) => fr[a] - fr[b])) out.push(`| ${fr[f]} | ${name(f)} | ${P.map((x) => n(x[f]?.[key])).join(' | ')} | ${n(g(f))} |`);
};
metric('Cold start, spawn to first 200 on `/`', 'coldMs', 'ms');
metric('RSS after load, all processes of the server', 'rssMb', 'MB');

const w = (f, r) => cell(f, r);
const wisp = Object.keys(finalRank.plaintext).find((f) => name(f) === 'Wisp');
out.push('', '## Wisp outside the top 3', '', ROUTES.filter((r) => finalRank[r][wisp] == null || finalRank[r][wisp] > 3).map((r) => `- ${LABEL[r]}: ${finalRank[r][wisp] == null ? 'unranked (too few clean passes)' : 'rank ' + finalRank[r][wisp]}`).join('\n') || 'none (by final value)');
out.push('', '## Cells where no two clean passes agree within 5%', '', flagged.length ? flagged.map((x) => `- ${x}`).join('\n') : 'none');
console.log(out.join('\n'));
