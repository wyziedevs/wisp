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
export async function runOnce({ run, idle, run_no }) {
  const drains = [];
  const { r, tries } = await redoResets(async () => { drains.push(await idle()); return run(); }, resetOnly);
  const redone = tries.length > 1;
  return {
    r, tries, drains,
    resets: redone ? tries.slice(0, -1).reduce((n, x) => n + x.bad, 0) : 0,
    redo: redone ? { run: run_no, tries: tries.map((x) => ({ rps: Math.round(x.rps), bad: x.bad })) } : undefined,
  };
}

// The results-file cell of one app and route. Any failed request: no req/s (null), `failed: true`.
export function cellRecord(rs, { resets, redos, drains }) {
  const c = cellOf(rs);
  return { rps: c.rps == null ? null : Math.round(c.rps), p99: c.p99, runs: rs.map((r) => Math.round(r.rps)), bad: c.bad, ...(resets && { resets }), ...(redos.length && { redos }), drain_s: drains, ...(c.failed && { errors: rs.map((r) => r.errs), failed: true }) };
}

// Validity, derived from a results file's data (a stored `valid` flag is never read).
// Why a host file cannot be ranked, or null: steal data recorded by mark.mjs, mean at most 10%.
export const hostWhy = (r) => (!r.steal ? 'no steal data' : r.steal.mean > 10 ? `steal mean ${r.steal.mean}% over 10%` : null);
// The drain marker of a cell, or null: -1 = never reached idle, none recorded = not measured.
export const drainWhy = (c) => (!Array.isArray(c.drain_s) || !c.drain_s.length ? 'no drain' : c.drain_s.some((d) => d < 0) ? 'not idle' : null);
// Why a cell is not ranked, or null: a failed request, fewer than 3 runs, or a drain that did not reach idle.
export const cellWhy = (c) => (c.failed || c.bad || c.rps == null ? 'failed requests' : c.runs?.length < 3 ? `${c.runs.length} valid runs` : drainWhy(c));
