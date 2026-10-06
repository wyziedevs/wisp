// node mark.mjs <host> <vmstat log>: records the steal readings taken during a
// host's run in results/<host>.json; valid is true when mean steal is 10% or less.
import { readFileSync, writeFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const [host, log] = process.argv.slice(2);
const f = join(dirname(fileURLToPath(import.meta.url)), 'results', `${host}.json`);
const r = JSON.parse(readFileSync(f, 'utf8'));
const st = readFileSync(log, 'utf8').split('\n').map((l) => l.trim().split(/\s+/)).filter((c) => c.length >= 17 && /^\d+$/.test(c[0])).map((c) => Number(c[15]));
const mean = st.reduce((a, b) => a + b, 0) / (st.length || 1);
r.steal = { mean: Math.round(mean * 100) / 100, max: Math.max(0, ...st), samples: st.length };
try { r.ip_local_port_range = readFileSync('/proc/sys/net/ipv4/ip_local_port_range', 'utf8').trim().replace(/\s+/g, ' '); } catch {}
r.valid = st.length > 0 && mean <= 10;
if (!r.valid) r.invalid = `steal mean ${r.steal.mean}% during the run (over 10%)`;
else delete r.invalid;
writeFileSync(f, JSON.stringify(r, null, 1));
console.log(host, 'steal mean', r.steal.mean, 'max', r.steal.max, r.valid ? 'valid' : 'INVALID');
