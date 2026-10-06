// node --test bench/rank
import test from 'node:test';
import assert from 'node:assert/strict';
import { sumGroupTicks, runOnce, cellRecord, resetOnly } from './lib.mjs';

const stat = (pid, pgrp, u, s) => `${pid} (a b) S 1 ${pgrp} ${pid} 0 -1 0 0 0 0 0 ${u} ${s} 0 0 20 0 1 0 1 1 1`;

test('sumGroupTicks adds user+system ticks of the processes in the group only, tolerating junk', () => {
  const t = sumGroupTicks(7, [stat(7, 7, 10, 5), stat(8, 7, 3, 1), stat(9, 9, 100, 100), () => { throw new Error('gone'); }, 'garbage']);
  assert.equal(t, 19);
});

const r = (rps, bad = 0, errs = {}) => ({ rps, p99: 1, bad, errs });
test('runOnce drains before every try, records redos, keeps the last, 3 tries at most', async () => {
  const seq = [r(100, 4, { 'connection error': 4 }), r(200, 2, { 'connection error': 2 }), r(300, 1, { 'connection error': 1 }), r(999)];
  let n = 0, drains = 0;
  const o = await runOnce({ run: async () => seq[n++], idle: async () => (++drains, drains), run_no: 3 });
  assert.equal(n, 3);
  assert.equal(o.r.rps, 300);
  assert.deepEqual(o.drains, [1, 2, 3]);
  assert.equal(o.tries.length, 3);
  assert.deepEqual(o.redo, { run: 3, tries: [{ rps: 100, bad: 4 }, { rps: 200, bad: 2 }, { rps: 300, bad: 1 }] });
  assert.equal(o.resets, 6);
});

test('runOnce: a clean run has one drain, no redo; a non-reset failure is not redone', async () => {
  const a = await runOnce({ run: async () => r(5), idle: async () => 0, run_no: 1 });
  assert.deepEqual(a.drains, [0]);
  assert.equal(a.redo, undefined);
  assert.equal(a.resets, 0);
  let n = 0;
  const b = await runOnce({ run: async () => (n++, r(1, 3, { 'connection error': 2, 'status 500': 1 })), idle: async () => 0, run_no: 1 });
  assert.equal(n, 1);
  assert.ok(!resetOnly(b.r));
});

test('cellRecord keeps drain_s per try, redos, resets and fails on any bad request', () => {
  const ok = cellRecord([r(10), r(30), r(20)], { resets: 0, redos: [], drains: [0, 0, 2] });
  assert.equal(ok.rps, 20);
  assert.deepEqual(ok.drain_s, [0, 0, 2]);
  assert.ok(!('resets' in ok) && !('redos' in ok));
  const f = cellRecord([r(10), r(30, 2, { x: 2 })], { resets: 5, redos: [{ run: 1, tries: [] }], drains: [-1, 0] });
  assert.equal(f.rps, null);
  assert.equal(f.failed, true);
  assert.equal(f.resets, 5);
  assert.equal(f.redos.length, 1);
  assert.deepEqual(f.drain_s, [-1, 0]);
});
