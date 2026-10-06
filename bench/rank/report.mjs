// Renders results/<host>.json as Markdown: a table per host (every framework,
// req/s with its place per route, cold start, memory), then every cell where
// Wisp is below 3rd or behind Hono. node report.mjs [results dir]
// A host file with an `invalid` field prints that reason instead of its table.
// Validity is derived from the data, never from a stored `valid` flag (lib.mjs): a host is ranked only
// with steal data at 10% mean or less; a cell only with no failed request, 3 runs and every drain
// before a try having reached idle. A cell that fails prints its req/s with the reason, unranked; a
// drain_s of -1 prints 'not idle', a missing one 'no drain', in every table.
import { readFileSync, existsSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { coldNote } from '../edge/util.mjs';
import { hostWhy, cellWhy, drainWhy } from './lib.mjs';
import { fileURLToPath } from 'node:url';

// Runs redone after a reset-only run (rank.mjs): every app's count, so the reader sees how often it happened.
const resetNote = (r) => {
  const n = Object.entries(r.cells).filter(([, c]) => c.resets).map(([k, c]) => `${k} ${c.resets}${c.redos ? ' (' + c.redos.map((d) => `run ${d.run}: ${d.tries.map((t) => n0(t.rps)).join(' then ')} req/s`).join('; ') + ')' : ''}`);
  return `; connection resets tolerated: redo up to 3 (3 tries per run, the last try kept, the same rule for every app)${n.length ? `; runs redone after connection resets (requests reset): ${n.join(', ')}` : ''}`;
};
// A host file records its wait-for-idle rule; an older file did not, and says so.
const drainNote = (r) => `; ${r.drain ? `before every run: ${r.drain}` : 'no wait-for-idle before runs (not recorded in this file)'}`;
// Per-run steal (mark.mjs): the worst run's steal, or the label for a file that has none.
const stealNote = (r) => { const s = Object.values(r.cells).map((c) => c.steal_runs); return s.every((x) => Array.isArray(x)) ? `; per-run steal max ${Math.max(0, ...s.flat().filter((x) => x != null))}%` : '; no per-run steal (not recorded in this file)'; };
// Provenance (rank.mjs `prov`): the Wisp commit and build date, runtime and contender versions, kernel.
const provNote = (r) => {
  const p = r.prov;
  if (!p) return 'provenance not recorded in this file';
  const v = Object.entries(p.versions || {}).filter(([, x]) => x).map(([k, x]) => `${k} ${x}`);
  const c = Object.values(p.contenders || {}).flatMap((o) => Object.entries(o).map(([k, x]) => `${k} ${x}`));
  return `Wisp ${p.wisp_commit || 'unknown'} built ${p.build_date || 'unknown'}; kernel ${p.kernel || 'unknown'}; ${v.join(', ') || 'no runtime versions'}; contenders ${[...new Set(c)].join(', ') || 'unrecorded'}`;
};
const dir = process.argv[2] || join(dirname(fileURLToPath(import.meta.url)), 'results');
const hosts = ['workerd', 'node', 'bun', 'deno'];
const routeNames = { '/': '`/`', '/list1000': '`/list1000`', '/json-big': '`/json-big`', '/params/42?q=hello%20world&x=1': '`/params`' };
const isWisp = (n) => n.startsWith('wisp');
const label = { workerd: 'workerd', node: 'Node', bun: 'Bun', deno: 'Deno' };
const n0 = (x) => Math.round(x).toLocaleString('en-US'); // req/s are stored exact; shown rounded
const losses = [];
let anyValid = false;
const out = [];

for (const host of hosts) {
  const f = join(dir, `${host}.json`);
  if (!existsSync(f)) continue;
  const r = JSON.parse(readFileSync(f, 'utf8'));
  if (r.invalid) { out.push(`### ${label[host]}\n`); out.push(`No valid run (${r.invalid}). The ${r.when.slice(0, 10)} numbers are not published; the rank table is pending a valid run.\n`); continue; }
  const whyHost = hostWhy(r), valid = !whyHost;
  anyValid ||= valid;
  const names = [...new Set(Object.keys(r.cells).map((k) => k.split(' /')[0]))];
  const routes = Object.keys(routeNames).filter((p) => names.some((n) => r.cells[`${n} ${p}`]));
  // metrics: value per framework, higher or lower better
  const metrics = routes.map((p) => ({ id: p, title: routeNames[p], better: 'high', val: (n) => { const c = r.cells[`${n} ${p}`]; return c && !cellWhy(c) ? c.rps : null; }, fmt: (v) => n0(v) }));
  metrics.push({ id: 'cold', title: 'cold start ms', better: 'low', val: (n) => r.cold[n]?.median, fmt: (v) => v.toFixed(0) });
  metrics.push({ id: 'rss', title: 'RSS MB after load', better: 'low', val: (n) => r.rss[n]?.load, fmt: (v) => String(v) });

  // place: Wisp variants are ranked as one entry each against the other frameworks only
  const place = (m, n) => {
    const v = m.val(n);
    if (v == null) return null;
    const rivals = names.filter((x) => x !== n && m.val(x) != null && (!isWisp(n) ? !isWisp(x) || x === bestWisp(m) : !isWisp(x)));
    return 1 + rivals.filter((x) => (m.better === 'high' ? m.val(x) > v : m.val(x) < v)).length;
  };
  const best = (m, list, i = 0) => list.filter((x) => m.val(x) != null).sort((a, b) => (m.better === 'high' ? m.val(b) - m.val(a) : m.val(a) - m.val(b)))[i];
  const bestWisp = (m) => best(m, names.filter(isWisp));

  const order = (n) => { const ps = metrics.slice(0, routes.length).map((m) => place(m, n)).filter((x) => x != null); return ps.reduce((a, b) => a + b, 0) / (ps.length || 1); };
  const rows = valid ? [...names].sort((a, b) => order(a) - order(b)) : [...names];
  out.push(`### ${label[host]}\n`);
  out.push(`c=${r.conns}, ${r.secs} s runs, median of ${r.runs}, ${coldNote(r.colds, r.cold)}; ${r.when.slice(0, 10)}${r.steal ? `, CPU steal mean ${r.steal.mean}% (max ${r.steal.max}%)` : ''}${r.ip_local_port_range ? `, ip_local_port_range ${r.ip_local_port_range}` : ''}${resetNote(r)}${drainNote(r)}${stealNote(r)}. ${valid ? 'Each cell: req/s (place among the frameworks; a second Wisp variant is not counted against the first; a cell that is not idle, has no drain, fewer than 3 runs, steal over 10% in a run or failed requests is shown unranked with the reason).' : `Each cell: req/s. Not ranked: ${whyHost}.`}\n`);
  out.push(`Provenance: ${provNote(r)}.
`);
  out.push(`| framework | ${metrics.map((m) => m.title).join(' | ')} |`);
  out.push(`|---${'|---'.repeat(metrics.length)}|`);
  for (const n of rows) {
    const cells = metrics.map((m) => {
      const v = m.val(n), c = m.id in routeNames ? r.cells[`${n} ${m.id}`] : null;
      if (v == null) {
        if (!c) return 'n/a';
        if (c.bad || c.failed || c.rps == null) return 'Failed'; // a cell with failed requests is shown, never ranked
        const why = valid ? cellWhy(c) : drainWhy(c);
        return isWisp(n) ? `**${m.fmt(c.rps)} (${valid ? 'unranked: ' : ''}${why})**` : `${m.fmt(c.rps)} (${valid ? 'unranked: ' : ''}${why})`;
      }
      const p = place(m, n);
      const s = valid ? `${m.fmt(v)} (#${p})` : c && drainWhy(c) ? `${m.fmt(v)} (${drainWhy(c)})` : m.fmt(v);
      return isWisp(n) ? `**${s}**` : s;
    });
    out.push(`| ${isWisp(n) ? `**${n}**` : n} | ${cells.join(' | ')} |`);
  }
  for (const [n, why] of Object.entries(r.failed)) out.push(`| ${n} | ${why.startsWith('missing') || why === 'does not start' ? why : 'wrong output, not ranked'} |`);
  out.push('');

  for (const n of valid ? names.filter(isWisp) : []) {
    for (const m of metrics) {
      const v = m.val(n), h = m.val('hono');
      if (v == null) continue;
      const p = place(m, n);
      const worse = (a, b) => (m.better === 'high' ? (b - a) / b : (a - b) / b) * 100; // % worse than b
      const behindHono = h != null && (m.better === 'high' ? v < h : v > h);
      if (p > 3 || behindHono) {
        const third = best(m, names.filter((x) => !isWisp(x)), 2);
        losses.push({ host: label[host], n, metric: m.title, p, v: m.fmt(v), h: h != null ? m.fmt(h) : '-', gapHono: h != null ? worse(v, h) : null, gap3: p > 3 && third ? worse(v, m.val(third)) : null, third });
      }
    }
  }
}

if (!anyValid) { console.log(out.join('\n')); process.exit(0); }
out.push('### Where Wisp is below 3rd or behind Hono\n');
out.push('Gap is how far behind in %: lower req/s, or higher cold start and memory. Gap to 3rd is against the framework in 3rd place (Wisp not counted).\n');
if (!losses.length) out.push('None.');
else {
  out.push('| host | Wisp variant | metric | place | Wisp | Hono | behind Hono | behind 3rd |');
  out.push('|---|---|---|---|---|---|---|---|');
  for (const l of losses) out.push(`| ${l.host} | ${l.n} | ${l.metric} | #${l.p} | ${l.v} | ${l.h} | ${l.gapHono != null && l.gapHono > 0 ? l.gapHono.toFixed(0) + '%' : 'ahead'} | ${l.gap3 != null ? l.gap3.toFixed(0) + '% (' + l.third + ')' : '-'} |`);
}
console.log(out.join('\n'));
