// node finalize.mjs <host> <attempt files...>: results/<host>.json = the LAST attempt; it is published (ranked) only when
// that attempt is fully valid (host steal mean <=10%, every cell ranked); otherwise it is marked invalid. Every attempt
// is listed in `attempts`. Never the best of several, never mixed.
import { readFileSync, writeFileSync } from "node:fs";
import { hostWhy, cellWhy } from "./lib.mjs";
const [host, ...files] = process.argv.slice(2);
const rs = files.map((f) => JSON.parse(readFileSync(f, "utf8")));
const info = rs.map((r) => ({ when: r.when, steal_mean: r.steal?.mean, steal_max: r.steal?.max, run_steal_max: Math.max(0, ...Object.values(r.cells).flatMap((c) => c.steal_runs || []).filter((x) => x != null)), unranked_cells: Object.values(r.cells).filter((c) => cellWhy(c)).length, host_why: hostWhy(r) }));
const last = rs[rs.length - 1];
const l = info[info.length - 1];
const ok = !l.host_why && l.unranked_cells === 0 && !Object.keys(last.failed).length;
last.attempts = info;
if (ok) { delete last.invalid; last.valid = true; } else { last.valid = false; last.invalid = `no complete attempt had every run at 10% steal or less (${info.length} attempts; last: host steal mean ${l.steal_mean}%, per-run steal max ${l.run_steal_max}%, ${l.unranked_cells} cells unranked)`; }
writeFileSync(new URL(`./results/${host}.json`, import.meta.url), JSON.stringify(last, null, 1));
console.log(host, ok ? "VALID" : "INVALID", JSON.stringify(l));
