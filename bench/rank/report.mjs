// Renders results/<host>.json as Markdown: a table per host (every framework,
// req/s with its place per route, cold start, memory), then every cell where
// Wisp is below 3rd or behind Hono. node report.mjs [results dir]
// A host file with an `invalid` field prints that reason instead of its table.
import { readFileSync, existsSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const dir = process.argv[2] || join(dirname(fileURLToPath(import.meta.url)), 'results');
const hosts = ['workerd', 'node', 'bun', 'deno'];
const routeNames = { '/': '`/`', '/list1000': '`/list1000`', '/json-big': '`/json-big`', '/params/42?q=hello%20world&x=1': '`/params`' };
const isWisp = (n) => n.startsWith('wisp');
const label = { workerd: 'workerd', node: 'Node', bun: 'Bun', deno: 'Deno' };
const n0 = (x) => x.toLocaleString('en-US');
const losses = [];
const out = [];

for (const host of hosts) {
  const f = join(dir, `${host}.json`);
  if (!existsSync(f)) continue;
  const r = JSON.parse(readFileSync(f, 'utf8'));
  if (r.invalid) { out.push(`### ${label[host]}\n`); out.push(`No valid run (${r.invalid}). The ${r.when.slice(0, 10)} numbers are not published; the rank table is pending a valid run.\n`); continue; }
  const names = [...new Set(Object.keys(r.cells).map((k) => k.split(' /')[0]))];
  const routes = Object.keys(routeNames).filter((p) => names.some((n) => r.cells[`${n} ${p}`]));
  // metrics: value per framework, higher or lower better
  const metrics = routes.map((p) => ({ id: p, title: routeNames[p], better: 'high', val: (n) => r.cells[`${n} ${p}`]?.rps, fmt: (v) => n0(v) }));
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
  const rows = [...names].sort((a, b) => order(a) - order(b));
  out.push(`### ${label[host]}\n`);
  out.push(`c=${r.conns}, ${r.secs} s runs, median of ${r.runs}, cold start median of ${r.colds}; ${r.when.slice(0, 10)}. Each cell: req/s (place among the frameworks; a second Wisp variant is not counted against the first).\n`);
  out.push(`| framework | ${metrics.map((m) => m.title).join(' | ')} |`);
  out.push(`|---${'|---'.repeat(metrics.length)}|`);
  for (const n of rows) {
    const cells = metrics.map((m) => {
      const v = m.val(n);
      if (v == null) return 'n/a';
      const p = place(m, n);
      const s = `${m.fmt(v)} (#${p})`;
      return isWisp(n) ? `**${s}**` : s;
    });
    out.push(`| ${isWisp(n) ? `**${n}**` : n} | ${cells.join(' | ')} |`);
  }
  for (const [n, why] of Object.entries(r.failed)) out.push(`| ${n} | ${why.startsWith('missing') || why === 'does not start' ? why : 'wrong output, not ranked'} |`);
  out.push('');

  for (const n of names.filter(isWisp)) {
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

if (!out.some((l) => l.startsWith('| framework'))) { console.log(out.join('\n')); process.exit(0); }
out.push('### Where Wisp is below 3rd or behind Hono\n');
out.push('Gap is how far behind in %: lower req/s, or higher cold start and memory. Gap to 3rd is against the framework in 3rd place (Wisp not counted).\n');
if (!losses.length) out.push('None.');
else {
  out.push('| host | Wisp variant | metric | place | Wisp | Hono | behind Hono | behind 3rd |');
  out.push('|---|---|---|---|---|---|---|---|');
  for (const l of losses) out.push(`| ${l.host} | ${l.n} | ${l.metric} | #${l.p} | ${l.v} | ${l.h} | ${l.gapHono != null && l.gapHono > 0 ? l.gapHono.toFixed(0) + '%' : 'ahead'} | ${l.gap3 != null ? l.gap3.toFixed(0) + '% (' + l.third + ')' : '-'} |`);
}
console.log(out.join('\n'));
