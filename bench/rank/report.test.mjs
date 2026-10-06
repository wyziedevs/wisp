// node --test bench/rank
import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';

test('a cell with failed requests prints Failed and is not ranked; cold n is stated', () => {
  const d = mkdtempSync(join(tmpdir(), 'rank-'));
  const cell = (rps, bad = 0) => ({ rps, p99: 1, runs: [rps, rps, rps], drain_s: [0, 0, 0], bad, ...(bad && { failed: true }) });
  writeFileSync(join(d, 'node.json'), JSON.stringify({
    host: 'node', when: '2026-01-01T00:00:00Z', secs: '1', runs: 1, conns: '64', colds: 15, steal: { mean: 0, max: 0, samples: 3, col: 'st' },
    cells: { 'wisp raw /': cell(900), 'hono /': cell(null, 5), 'fastify /': cell(500) },
    cold: { 'wisp raw': { median: 10, min: 9, n: 15 }, hono: { median: 20, min: 18, n: 11 }, fastify: { median: 30, min: 29, n: 15 } },
    rss: {}, failed: {},
  }));
  const out = execFileSync(process.execPath, [join(dirname(fileURLToPath(import.meta.url)), 'report.mjs'), d]).toString();
  assert.match(out, /\| hono \| Failed \|/);
  assert.match(out, /\*\*900 \(#1\)\*\*/);
  assert.match(out, /fastify \| 500 \(#2\)/);
  assert.match(out, /11 to 15 of 15/);
  assert.match(out, /connection resets tolerated: redo up to 3/);
  assert.match(out, /no wait-for-idle before runs/);
});

test('a file with a recorded drain and redos states both', () => {
  const d = mkdtempSync(join(tmpdir(), 'rank-'));
  writeFileSync(join(d, 'node.json'), JSON.stringify({
    host: 'node', when: '2026-01-01T00:00:00Z', secs: '1', runs: 1, conns: '64', colds: 15, drain: 'server under 5% of a core',
    cells: { 'wisp raw /': { rps: 900, p99: 1, runs: [900], bad: 0, resets: 4, redos: [{ run: 1, tries: [{ rps: 100, bad: 4 }, { rps: 900, bad: 0 }] }] } },
    cold: { 'wisp raw': { median: 10, min: 9, n: 15 } }, rss: {}, failed: {},
  }));
  const out = execFileSync(process.execPath, [join(dirname(fileURLToPath(import.meta.url)), 'report.mjs'), d]).toString();
  assert.match(out, /before every run: server under 5% of a core/);
  assert.match(out, /run 1: 100 then 900 req\/s/);
});

// Validity is derived from the data (steal, 3 runs, no failures, drain reached), never from a stored `valid` flag.
const render = (file) => {
  const d = mkdtempSync(join(tmpdir(), 'rank-'));
  writeFileSync(join(d, 'node.json'), JSON.stringify({ host: 'node', when: '2026-01-01T00:00:00Z', secs: '1', runs: 3, conns: '64', colds: 15, drain: 'x', cold: {}, rss: {}, failed: {}, ...file }));
  return execFileSync(process.execPath, [join(dirname(fileURLToPath(import.meta.url)), 'report.mjs'), d]).toString();
};
const good = (rps, drain = [0, 0, 0]) => ({ rps, p99: 1, runs: [rps, rps, rps], bad: 0, drain_s: drain });
const steal = (mean) => ({ mean, max: mean, samples: 9, col: 'st' });

test('a stored valid flag is not trusted: no steal data means no places', () => {
  const out = render({ valid: true, cells: { 'wisp raw /': good(900), 'hono /': good(500) } });
  assert.doesNotMatch(out, /#1/);
  assert.match(out, /Not ranked: no steal data/);
});

test('steal over 10% means no places even when the file says valid', () => {
  const out = render({ valid: true, steal: steal(12), cells: { 'wisp raw /': good(900), 'hono /': good(500) } });
  assert.doesNotMatch(out, /#1/);
  assert.match(out, /Not ranked: .*steal mean 12%/);
});

test('valid is derived from the data when the flag is absent', () => {
  const out = render({ steal: steal(0.5), cells: { 'wisp raw /': good(900), 'hono /': good(500) } });
  assert.match(out, /\*\*900 \(#1\)\*\*/);
  assert.match(out, /hono \| 500 \(#2\)/);
});

test('drain_s -1 shows not idle and the cell is unranked with the reason', () => {
  const out = render({ steal: steal(0), cells: { 'wisp raw /': good(900), 'hono /': good(950, [0, -1, 0]), 'fastify /': good(500) } });
  assert.match(out, /hono \| 950 \(unranked: not idle\)/);
  assert.match(out, /\*\*900 \(#1\)\*\*/);
  assert.match(out, /fastify \| 500 \(#2\)/);
});

test('missing drain_s shows no drain and is unranked; fewer than 3 runs is unranked', () => {
  const nod = { rps: 700, p99: 1, runs: [700, 700, 700], bad: 0 };
  const two = { rps: 800, p99: 1, runs: [800, 800], bad: 0, drain_s: [0, 0] };
  const out = render({ steal: steal(0), cells: { 'wisp raw /': good(900), 'hono /': nod, 'fastify /': two } });
  assert.match(out, /hono \| 700 \(unranked: no drain\)/);
  assert.match(out, /fastify \| 800 \(unranked: 2 valid runs\)/);
  assert.match(out, /\*\*900 \(#1\)\*\*/);
});

test('an unranked host still marks not idle / no drain per cell', () => {
  const out = render({ cells: { 'wisp raw /': good(900, [-1, 0, 0]), 'hono /': { rps: 500, p99: 1, runs: [500, 500, 500], bad: 0 } } });
  assert.match(out, /900 \(not idle\)/);
  assert.match(out, /500 \(no drain\)/);
});

// Per-run steal: a cell is ranked only if no run saw steal over 10%; old files say 'no per-run steal'.
test('a run with steal over 10% unranks its cell even when the host mean is fine', () => {
  const sr = (v) => ({ ...good(950), steal_runs: v });
  const out = render({ steal: steal(0.5), cells: { 'wisp raw /': { ...good(900), steal_runs: [0, 0, 1] }, 'hono /': sr([0, 25, 0]), 'fastify /': good(500) } });
  assert.match(out, /hono \| 950 \(unranked: steal 25% in a run\)/);
  assert.match(out, /\*\*900 \(#1\)\*\*/);
});

test('a cell without per-run steal is labelled, a run with no sample is unranked', () => {
  const out = render({ steal: steal(0), cells: { 'wisp raw /': { ...good(900), steal_runs: [0, 0, 0] }, 'hono /': { ...good(500), steal_runs: [0, null, 0] } } });
  assert.match(out, /hono \| 500 \(unranked: steal not sampled in a run\)/);
  const old = render({ steal: steal(0), cells: { 'wisp raw /': good(900) } });
  assert.match(old, /no per-run steal/);
  assert.match(old, /\*\*900 \(#1\)\*\*/);
});

test('fewer than 3 runs recorded is unranked even when runs is absent', () => {
  const out = render({ steal: steal(0), cells: { 'wisp raw /': good(900), 'hono /': { rps: 5, p99: 1, bad: 0, drain_s: [0, 0, 0] } } });
  assert.match(out, /hono \| 5 \(unranked: 0 valid runs\)/);
});

test('steal recorded by the old mark.mjs (iowait column) is not trusted: no places', () => {
  const out = render({ steal: { mean: 0, max: 2, samples: 9 }, cells: { 'wisp raw /': good(900), 'hono /': good(500) } });
  assert.doesNotMatch(out, /#1/);
  assert.match(out, /Not ranked: steal not read from the st column/);
  const ok = render({ steal: { mean: 0, max: 2, samples: 9, col: 'st' }, cells: { 'wisp raw /': good(900) } });
  assert.match(ok, /\*\*900 \(#1\)\*\*/);
});

test('a cell with failed requests is never ranked even if the file kept a req/s', () => {
  const out = render({ steal: steal(0), cells: { 'wisp raw /': good(900), 'hono /': { ...good(950), bad: 3 } } });
  assert.match(out, /hono \| Failed \|/);
  assert.match(out, /\*\*900 \(#1\)\*\*/);
});

test('report prints the provenance of a host file and exact rps rounds only for display', () => {
  const out = render({ steal: steal(1), prov: { wisp_commit: 'abc1234', build_date: '2026-10-06', kernel: '6.1.0-x', ip_local_port_range: '32768 60999', versions: { node: 'v22.1.0', bun: null }, contenders: { micro: { hono: '4.6.1', fastify: '5.0.0' } } },
    cells: { 'wisp raw /': good(1234.5678), 'hono /': good(500.4) } });
  assert.match(out, /Wisp abc1234/);
  assert.match(out, /built 2026-10-06/);
  assert.match(out, /kernel 6\.1\.0-x/);
  assert.match(out, /node v22\.1\.0/);
  assert.match(out, /hono 4\.6\.1/);
  assert.match(out, /\*\*1,235 \(#1\)\*\*/);
  assert.match(out, /hono \| 500 \(#2\)/);
});

test('a file without provenance says so', () => {
  assert.match(render({ steal: steal(1), cells: { 'wisp raw /': good(900) } }), /provenance not recorded/);
});
