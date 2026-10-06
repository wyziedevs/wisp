// node report.mjs [results dir]: ranks and a markdown table from results/<contender>.json (written by measure.mjs).
// Validity and ranks are derived from the data (lib.mjs), never from a stored flag.
import { readFileSync, readdirSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { median, rankCells, cellWhy, fmtMs, MIN_RUNS, MAX_STEAL } from './lib.mjs';

const dir = process.argv[2] || join(dirname(fileURLToPath(import.meta.url)), 'results');
const res = readdirSync(dir).filter((f) => f.endsWith('.json')).map((f) => JSON.parse(readFileSync(join(dir, f), 'utf8')));
const ORDER = ['wisp', 'sveltekit', 'next', 'nuxt', 'hono-node', 'hono-bun', 'express', 'fastify', 'axum', 'actix'];
res.sort((a, b) => ORDER.indexOf(a.contender) - ORDER.indexOf(b.contender));
const METRICS = [
  ['cold', 'Cold build (clean cache, deps downloaded)'],
  ['warm_noop', 'Warm build, nothing changed'],
  ['warm_edit', 'Warm build, one logic file edited'],
  ['warm_edit_markup', 'Warm build, one template file edited'],
  ['dev_logic', 'Dev reload, logic edit to new text over HTTP'],
  ['dev_markup', 'Dev reload, template edit to new text over HTTP'],
];
const used = METRICS.filter(([k]) => res.some((r) => r.cells?.[k]));
const ranks = Object.fromEntries(used.map(([k]) => [k, rankCells(Object.fromEntries(res.map((r) => [r.contender, r.cells?.[k]])))]));

const out = [];
const cellText = (r, k) => {
  const c = r.cells?.[k];
  const why = cellWhy(c);
  if (c?.ms?.length >= MIN_RUNS && !why) { const rk = ranks[k][r.contender]; const t = `${fmtMs(median(c.ms))} (#${rk})`; return rk === 1 ? `**${t}**` : t; }
  return `${c?.ms?.length ? `${fmtMs(median(c.ms))} ` : ''}unranked: ${why}`;
};
out.push('| contender | ' + used.map(([, t]) => t).join(' | ') + ' | artifact |', '|' + '---|'.repeat(used.length + 2));
for (const r of res) out.push(`| ${r.contender} | ${used.map(([k]) => cellText(r, k)).join(' | ')} | ${r.artifact ? `${(r.artifact.bytes / 1e6).toFixed(1)} MB ${r.artifact.what}` : 'n/a'} |`);

const w = res.find((r) => r.contender === 'wisp');
if (w) {
  const not3 = used.map(([k]) => [k, ranks[k].wisp]).filter(([, rk]) => rk > 3).map(([k, rk]) => `${k} (#${rk})`);
  const unr = used.map(([k]) => [k, cellWhy(w.cells?.[k])]).filter(([, y]) => y).map(([k, y]) => `${k} (${y})`);
  out.push('', not3.length ? `Wisp is not top 3: ${not3.join(', ')}.` : 'Wisp is top 3 in every ranked cell.');
  if (unr.length) out.push(`Wisp cells not ranked: ${unr.join(', ')}.`);
}

out.push('', `Ranks: lower median is better, ties share a rank, only cells with ${MIN_RUNS} or more valid runs. A valid run has CPU steal at most ${MAX_STEAL}% (/proc/stat st column over the run), the machine drained (under 5% of a core busy) before it, and ran under flock /tmp/wisp-bench.lock.`, '', '### All runs', '', '| contender | cell | runs (ms) | steal % per run | drain s | invalid tries |', '|---|---|---|---|---|---|');
for (const r of res) for (const [k] of used) {
  const c = r.cells?.[k];
  if (!c || c.na) { if (c?.na) out.push(`| ${r.contender} | ${k} | ${c.na} | | | |`); continue; }
  out.push(`| ${r.contender} | ${k} | ${c.ms.map(Math.round).join(', ')} | ${(c.steal || []).map((s) => (s == null ? '?' : s.toFixed(2))).join(', ')} | ${(c.drain_s || []).join(', ')} | ${(c.invalid || []).map((i) => `${Math.round(i.ms)}ms/${i.steal == null ? '?' : i.steal.toFixed(1)}%`).join(', ') || '0'} |`);
}
const h = res.find((r) => r.host)?.host;
if (h) out.push('', '### Host', '', '```', JSON.stringify(h, null, 1), '```');
console.log(out.join('\n'));
