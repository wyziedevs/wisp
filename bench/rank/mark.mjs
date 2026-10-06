// node mark.mjs <host> <vmstat -t log>: records the steal readings taken during a host's run in
// results/<host>.json: the host mean (valid when 10% or less), and per cell the mean steal of each
// timed run (`steal_runs`, from the run windows `win` that rank.mjs recorded). report.mjs derives
// validity from this data and never reads the `valid` flag kept here for older readers.
import { readFileSync, writeFileSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseVmstat, stealRuns } from './lib.mjs';

const [host, log] = process.argv.slice(2);
const f = join(dirname(fileURLToPath(import.meta.url)), 'results', `${host}.json`);
const r = JSON.parse(readFileSync(f, 'utf8'));
const samples = parseVmstat(readFileSync(log, 'utf8')).slice(1); // the first line is the average since boot
const st = samples.map((s) => s.st);
const mean = st.reduce((a, b) => a + b, 0) / (st.length || 1);
r.steal = { mean: Math.round(mean * 100) / 100, max: Math.max(0, ...st), samples: st.length, col: 'st' };
for (const c of Object.values(r.cells)) if (Array.isArray(c.win)) c.steal_runs = stealRuns(samples, c.win);
try { r.ip_local_port_range = readFileSync('/proc/sys/net/ipv4/ip_local_port_range', 'utf8').trim().replace(/\s+/g, ' '); } catch {}
r.valid = st.length > 0 && mean <= 10;
if (!r.valid) r.invalid = `steal mean ${r.steal.mean}% during the run (over 10%)`;
else delete r.invalid;
writeFileSync(f, JSON.stringify(r, null, 1));
console.log(host, 'steal mean', r.steal.mean, 'max', r.steal.max, r.valid ? 'valid' : 'INVALID');
