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

import { parseVmstat, stealRuns } from './lib.mjs';

test('runOnce records the window of the kept (last) try', async () => {
  const seq = [r(1, 2, { 'connection error': 2 }), r(2)];
  let n = 0, t = 100;
  const o = await runOnce({ run: async () => seq[n++], idle: async () => 0, run_no: 1, now: () => (t += 10) });
  assert.deepEqual(o.win, [130, 140]);
});

test('cellRecord stores the run windows as win', () => {
  const c = cellRecord([r(1), r(2), r(3)], { resets: 0, redos: [], drains: [0, 0, 0], wins: [[1, 2], [3, 4], [5, 6]] });
  assert.deepEqual(c.win, [[1, 2], [3, 4], [5, 6]]);
});

const T = (s) => `2026-01-01 00:00:${String(s).padStart(2, '0')}`;
const vm = (rows) => ['procs ---memory--- -cpu-------- -----timestamp-----', ' r  b swpd free buff cache si so bi bo in cs us sy id wa st gu UTC',
  ...rows.map(([s, st]) => ` 1  0 0 1 1 1 0 0 0 0 1 1 5 3 ${92 - st} 0 ${st} 0 ${T(s)}`)].join('\n');

test('parseVmstat reads steal and the timestamp; -t absent gives t null', () => {
  const a = parseVmstat(vm([[1, 4], [2, 0]]));
  assert.deepEqual(a.map((x) => x.st), [4, 0]);
  assert.equal(a[1].t - a[0].t, 1);
  const b = parseVmstat(" 1  0 0 1 1 1 0 0 0 0 1 1 5 3 92 0 7 0");
  assert.deepEqual(b, [{ t: null, st: 7 }]);
});

test('stealRuns is the mean steal per run window; a window with no sample is null', () => {
  const s = parseVmstat(vm([[1, 0], [2, 0], [3, 30], [4, 30], [5, 2], [6, 2]]));
  const t = (k) => s[k - 1].t;
  assert.deepEqual(stealRuns(s, [[t(1), t(2)], [t(3), t(4)], [t(5), t(6)], [t(6) + 100, t(6) + 110]]), [0, 30, 2, null]);
  assert.deepEqual(stealRuns(parseVmstat(' 1 0 0 1 1 1 0 0 0 0 1 1 5 3 92 0 7 0'), [[1, 2]]), [null]);
});

test('parseVmstat takes steal from the st column, not wa (header-driven; 16 without a header)', () => {
  const a = parseVmstat(vm([[1, 4]]).replace(/ 92 0 /, ' 88 6 '));
  assert.equal(a[0].st, 4); // wa is 6, st is 4
  const b = parseVmstat(' 1 0 0 1 1 1 0 0 0 0 1 1 5 3 88 6 4 0');
  assert.equal(b[0].st, 4);
});

import { npmVersions, provenance } from './lib.mjs';

test('cellRecord stores the exact median req/s, not a rounded one', () => {
  const c = cellRecord([r(100.25), r(300.75), r(200.5)], { resets: 0, redos: [], drains: [0, 0, 0] });
  assert.equal(c.rps, 200.5);
  assert.deepEqual(c.runs, [100.25, 300.75, 200.5]);
  const e = cellRecord([r(1.123456789), r(2.987654321), r(2.000000001)], { resets: 0, redos: [], drains: [0, 0, 0] });
  assert.equal(e.rps, 2.000000001);
});

test('npmVersions flattens `npm ls --depth=0 --json`', () => {
  const j = { name: 'x', dependencies: { hono: { version: '4.6.1' }, fastify: { version: '5.0.0' }, bad: {} } };
  assert.deepEqual(npmVersions(j), { hono: '4.6.1', fastify: '5.0.0' });
  assert.deepEqual(npmVersions({}), {});
  assert.deepEqual(npmVersions(null), {});
});

test('provenance gathers runtimes, kernel, port range and the build record; a failing probe is null', () => {
  const sh = { 'uname -r': '6.1.0-x\n', 'node --version': 'v22.1.0\n', 'bun --version': '1.2.3\n', 'deno --version': 'deno 2.1.0 (stable)\nv8 13\n' };
  const files = {
    '/out/provenance.json': JSON.stringify({ wisp_commit: 'abc1234', build_date: '2026-10-06', contenders: { micro: { hono: '4.6.1' } } }),
    '/proc/sys/net/ipv4/ip_local_port_range': '32768\t60999\n',
  };
  const p = provenance({
    out: '/out',
    exec: (c) => { if (c in sh) return sh[c]; throw new Error('no ' + c); },
    read: (f) => { if (f in files) return files[f]; throw new Error('no ' + f); },
    workerd: '/w/workerd',
  });
  assert.equal(p.wisp_commit, 'abc1234');
  assert.equal(p.build_date, '2026-10-06');
  assert.deepEqual(p.contenders, { micro: { hono: '4.6.1' } });
  assert.equal(p.kernel, '6.1.0-x');
  assert.equal(p.ip_local_port_range, '32768 60999');
  assert.deepEqual(p.versions, { node: 'v22.1.0', bun: '1.2.3', deno: '2.1.0', workerd: null });
});
