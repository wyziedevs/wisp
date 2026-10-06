// Pure parts of rank.mjs, testable without a VPS (lib.test.mjs).
import { statTicks, redoResets, cellOf } from '../edge/util.mjs';

// CPU ticks (user + system) of the processes of group `pgid`; `stats` are /proc/<pid>/stat texts
// (or functions that throw for a process that vanished). The server runs under setsid, so pgrp = pid.
export function sumGroupTicks(pgid, stats) {
  let sum = 0;
  for (const s of stats) {
    try { const t = statTicks(typeof s === 'function' ? s() : s); if (t.pgrp === pgid) sum += t.ticks; } catch {}
  }
  return sum;
}

// A run whose only failures are connection resets.
export const resetOnly = (r) => r.bad > 0 && Object.keys(r.errs).every((e) => e === 'connection error');

// One timed run of one app: the server drains (`idle()`, seconds waited or -1) before EVERY try, a
// reset-only run is redone (3 tries at most, the LAST try kept). Returns the kept run, each try's drain,
// the requests reset in the discarded tries and the redo record (undefined when nothing was redone).
export async function runOnce({ run, idle, run_no, now = () => Date.now() / 1000 }) {
  const drains = [];
  let win;
  const { r, tries } = await redoResets(async () => { drains.push(await idle()); const t0 = now(); const x = await run(); win = [t0, now()]; return x; }, resetOnly);
  const redone = tries.length > 1;
  return {
    r, tries, drains, win, // win: epoch seconds of the kept try, for the per-run steal mean (mark.mjs)
    resets: redone ? tries.slice(0, -1).reduce((n, x) => n + x.bad, 0) : 0,
    redo: redone ? { run: run_no, tries: tries.map((x) => ({ rps: Math.round(x.rps), bad: x.bad })) } : undefined,
  };
}

// The results-file cell of one app and route. Any failed request: no req/s (null), `failed: true`.
export function cellRecord(rs, { resets, redos, drains, wins }) {
  const c = cellOf(rs);
  return { rps: c.rps == null ? null : Math.round(c.rps), p99: c.p99, runs: rs.map((r) => Math.round(r.rps)), bad: c.bad, ...(resets && { resets }), ...(redos.length && { redos }), drain_s: drains, ...(wins && { win: wins }), ...(c.failed && { errors: rs.map((r) => r.errs), failed: true }) };
}

// Validity, derived from a results file's data (a stored `valid` flag is never read).
// Why a host file cannot be ranked, or null: steal data recorded by mark.mjs, mean at most 10%.
export const hostWhy = (r) => (!r.steal ? 'no steal data' : r.steal.col !== 'st' ? 'steal not read from the st column (older mark.mjs read wa)' : r.steal.mean > 10 ? `steal mean ${r.steal.mean}% over 10%` : null);
// The drain marker of a cell, or null: -1 = never reached idle, none recorded = not measured.
export const drainWhy = (c) => (!Array.isArray(c.drain_s) || !c.drain_s.length ? 'no drain' : c.drain_s.some((d) => d < 0) ? 'not idle' : null);
// Why a cell is not ranked, or null: a failed request, fewer than 3 runs, or a drain that did not reach idle.
export const cellWhy = (c) => (c.failed || c.bad || c.rps == null ? 'failed requests' : !(c.runs?.length >= 3) ? `${c.runs?.length ?? 0} valid runs` : drainWhy(c) || stealWhy(c));
// Steal during each timed run (mark.mjs stores `steal_runs`, the mean per run window): none = an older file.
export const stealWhy = (c) => (!Array.isArray(c.steal_runs) ? null : c.steal_runs.some((s) => s == null) ? 'steal not sampled in a run' : c.steal_runs.some((s) => s > 10) ? `steal ${Math.max(...c.steal_runs)}% in a run` : null);

// `vmstat -t 1` text -> [{ t: epoch seconds or null without -t, st: steal % }].
export function parseVmstat(text) {
  const out = [];
  let st = 16; // procps: r b swpd free buff cache si so bi bo in cs us sy id wa st (gu)
  for (const l of text.split('\n')) {
    const c = l.trim().split(/\s+/);
    if (c.includes('st') && c.includes('wa')) { st = c.indexOf('st'); continue; }
    if (c.length < 17 || !/^\d+$/.test(c[0])) continue;
    const i = c.findIndex((x) => /^\d{4}-\d\d-\d\d$/.test(x));
    const ms = i > 0 && c[i + 1] ? new Date(`${c[i]}T${c[i + 1]}`).getTime() : NaN;
    out.push({ t: ms === ms ? ms / 1000 : null, st: Number(c[st]) });
  }
  return out;
}

// Mean steal % of the 1 s samples stamped inside each [t0, t1] window (epoch seconds); null when none.
export function stealRuns(samples, wins) {
  return wins.map(([a, b]) => {
    const s = samples.filter((x) => x.t != null && x.t >= a && x.t <= b);
    return s.length ? Math.round((s.reduce((n, x) => n + x.st, 0) / s.length) * 100) / 100 : null;
  });
}
