// node --test bench/build
import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';
import { median, parseCpu, stealPct, busyPerSec, validRun, editText, cellWhy, rankCells, MIN_RUNS } from './lib.mjs';

const here = dirname(fileURLToPath(import.meta.url));

test('median: odd, even, unsorted', () => {
  assert.equal(median([5, 1, 3]), 3);
  assert.equal(median([4, 1, 3, 2]), 2.5);
  assert.equal(median([]), null);
});

test('parseCpu reads fields 2..9 of the cpu line, steal is the last', () => {
  const c = parseCpu('cpu  10 1 20 300 4 5 6 7 0 0\ncpu0 1 1 1 1 1 1 1 1 0 0\n');
  assert.deepEqual(c, { user: 10, nice: 1, system: 20, idle: 300, iowait: 4, irq: 5, softirq: 6, steal: 7, total: 353 });
});

test('stealPct is steal over total between two snapshots', () => {
  const a = parseCpu('cpu  0 0 0 100 0 0 0 0');
  const b = parseCpu('cpu  0 0 0 180 0 0 0 20');
  assert.equal(stealPct(a, b), 20);
  assert.equal(stealPct(a, a), 0);
});

test('busyPerSec counts user..softirq jiffies, not idle, iowait or steal', () => {
  const a = parseCpu('cpu  0 0 0 0 0 0 0 0');
  const b = parseCpu('cpu  10 0 10 300 50 0 5 90');
  assert.equal(busyPerSec(a, b, 5), 5); // 25 busy jiffies over 5 s
});

test('a run over 10% steal is invalid, exactly 10% is valid', () => {
  assert.equal(validRun({ steal: 10 }), true);
  assert.equal(validRun({ steal: 10.01 }), false);
  assert.equal(validRun({ steal: null }), false);
});

test('editText appends a version to the first occurrence, always from the original', () => {
  assert.equal(editText('a Hello, World! b Hello, World!', 'Hello, World!', 3), 'a Hello, World! v3 b Hello, World!');
});

test('cellWhy: fewer than MIN_RUNS valid runs, a run over 10% steal, no drain', () => {
  assert.equal(MIN_RUNS, 3);
  const ok = { ms: [1, 2, 3], steal: [0, 0, 0], drain_s: [0, 0, 0] };
  assert.equal(cellWhy(ok), null);
  assert.match(cellWhy({ ...ok, ms: [1, 2] }), /2 valid runs/);
  assert.match(cellWhy({ ...ok, steal: [0, 11, 0] }), /steal 11%/);
  assert.match(cellWhy({ ...ok, drain_s: [0, -1, 0] }), /not idle/);
  assert.match(cellWhy({ ...ok, drain_s: undefined }), /no drain/);
  assert.match(cellWhy({ na: 'no build step' }), /no build step/);
  assert.match(cellWhy(undefined), /not measured/);
});

test('rankCells ranks by median (lower is better), ties share a rank, unranked cells get none', () => {
  const c = (ms) => ({ ms, steal: [0, 0, 0], drain_s: [0, 0, 0] });
  const r = rankCells({ a: c([10, 10, 10]), b: c([5, 5, 5]), c: c([10, 10, 10]), d: c([1, 1]), e: { na: 'x' } });
  assert.deepEqual(r, { b: 1, a: 2, c: 2 });
});

test('report.mjs prints a ranked table, flags where Wisp is not top 3, and states unranked cells', () => {
  const d = mkdtempSync(join(tmpdir(), 'bb-'));
  const cell = (m) => ({ ms: [m, m + 1, m + 2], steal: [0, 1, 0], drain_s: [0, 0, 0], tries: 3 });
  const mk = (name, ms) => writeFileSync(join(d, `${name}.json`), JSON.stringify({ contender: name, host: { nproc: 4 }, cells: { cold: cell(ms), warm_noop: cell(ms) }, artifact: { bytes: 100, what: 'bin' } }));
  mk('wisp', 900); mk('axum', 100); mk('actix', 200); mk('next', 300); mk('nuxt', 50);
  writeFileSync(join(d, 'express.json'), JSON.stringify({ contender: 'express', host: { nproc: 4 }, cells: { cold: { na: 'no build step' }, warm_noop: { na: 'no build step' } } }));
  const out = execFileSync(process.execPath, [join(here, 'report.mjs'), d]).toString();
  assert.match(out, /\| wisp \| 901 ms \(#5\)/);
  assert.match(out, /\| nuxt \| .*\(#1\)/);
  assert.match(out, /\| express \| .*no build step/);
  assert.match(out, /Wisp is not top 3: cold \(#5\)/);
});
