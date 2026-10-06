// Pure parts of the build-time bench, testable without a VPS (lib.test.mjs).

export const MIN_RUNS = 3; // a cell is ranked only from this many valid runs
export const MAX_STEAL = 10; // a run with more steal (% of all CPU time, /proc/stat st column) is invalid
export const MAX_TRIES = 3; // an invalid run is retried this many times, then the cell is published without it

export function median(a) {
  if (!a.length) return null;
  const s = [...a].sort((x, y) => x - y);
  const m = s.length >> 1;
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
}

// First `cpu ` line of /proc/stat -> jiffies. Fields 2..9: user nice system idle iowait irq softirq steal.
export function parseCpu(text) {
  const f = text.split('\n').find((l) => l.startsWith('cpu ')).trim().split(/\s+/).slice(1, 9).map(Number);
  const [user, nice, system, idle, iowait, irq, softirq, steal] = f;
  return { user, nice, system, idle, iowait, irq, softirq, steal, total: f.reduce((a, b) => a + b, 0) };
}

// Steal as % of all CPU time between two snapshots; 0 when no time passed.
export const stealPct = (a, b) => (b.total === a.total ? 0 : (100 * (b.steal - a.steal)) / (b.total - a.total));

// Busy jiffies per second between two snapshots: everything but idle, iowait and steal. 100 Hz, so 5 = 5% of a core.
export const busyPerSec = (a, b, secs) => ((b.user - a.user) + (b.nice - a.nice) + (b.system - a.system) + (b.irq - a.irq) + (b.softirq - a.softirq)) / secs;

export const IDLE_BUSY = 5; // drained = under 5% of a core busy, machine wide

export const validRun = (r) => r.steal != null && r.steal <= MAX_STEAL;

// The text of a file after edit number `i`: a version tag after the first `base`, from the ORIGINAL text,
// so every edit changes the file and the tag is what the poll waits for.
export const editText = (orig, base, i) => orig.replace(base, `${base} v${i}`);

// Why a cell is not ranked, or null.
export function cellWhy(c) {
  if (!c) return 'not measured';
  if (c.na) return c.na;
  const n = c.ms?.length ?? 0;
  if (n < MIN_RUNS) return `${n} valid runs`;
  if (!Array.isArray(c.steal) || c.steal.length !== n) return 'no steal data';
  const bad = c.steal.filter((s) => s == null || s > MAX_STEAL);
  if (bad.length) return `steal ${Math.max(...bad)}% in a run`;
  if (!Array.isArray(c.drain_s) || !c.drain_s.length) return 'no drain';
  if (c.drain_s.some((d) => d < 0)) return 'not idle';
  return null;
}

// { contender: rank } for the ranked cells of one metric; lower median is better, ties share a rank.
export function rankCells(cells) {
  const e = Object.entries(cells).filter(([, c]) => !cellWhy(c)).map(([k, c]) => [k, median(c.ms)]).sort((x, y) => x[1] - y[1]);
  const out = {};
  e.forEach(([k, m], i) => { out[k] = i && m === e[i - 1][1] ? out[e[i - 1][0]] : i + 1; });
  return out;
}

export const fmtMs = (ms) => (ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(2)} s`);
