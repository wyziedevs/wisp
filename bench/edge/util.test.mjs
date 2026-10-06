// node --test bench/edge
import test from 'node:test';
import assert from 'node:assert/strict';
import { redoResets, statTicks, waitIdle, failedCount, cellOf, cellValid, badReply, coldOf, coldNote } from './util.mjs';

test('failedCount sums requests, not distinct codes', () => {
  assert.equal(failedCount({ statusCodeDistribution: { 200: 900, 500: 40, 404: 2 } }), 42);
  assert.equal(failedCount({ statusCodeDistribution: { 200: 9 }, errorDistribution: { 'connection closed': 3 } }), 3);
  assert.equal(failedCount({ statusCodeDistribution: { 200: 9 } }), 0);
  assert.equal(failedCount({ statusCodeDistribution: { 200: 9 }, errorDistribution: { 'aborted due to deadline': 64 } }), 0);
});

test('deadline aborts are the in-flight requests: more than the connection count is a failure', () => {
  const j = (n) => ({ statusCodeDistribution: { 200: 9 }, errorDistribution: { 'aborted due to deadline': n } });
  assert.equal(failedCount(j(64), 64), 0);
  assert.equal(failedCount(j(65), 64), 65);
  assert.equal(failedCount(j(65), '64'), 65);
});

test('a cell with any failed request has no req/s', () => {
  const ok = cellOf([{ rps: 10, p99: 1, bad: 0 }, { rps: 30, p99: 3, bad: 0 }, { rps: 20, p99: 2, bad: 0 }]);
  assert.deepEqual(ok, { bad: 0, rps: 20, p99: 2 });
  const f = cellOf([{ rps: 10, p99: 1, bad: 0 }, { rps: 99, p99: 1, bad: 7 }]);
  assert.equal(f.rps, null);
  assert.equal(f.bad, 7);
  assert.ok(f.failed);
  assert.ok(cellValid(ok));
  assert.ok(!cellValid(f));
  assert.ok(!cellValid({ rps: 5, bad: 1 }));
});

test('badReply checks status and body', () => {
  assert.equal(badReply('/', 200, 'hello'), null);
  assert.match(badReply('/', 200, 'hullo'), /wrong body/);
  assert.match(badReply('/', 500, 'hello'), /status 500/);
  assert.equal(badReply('/json', 200, '{"ok":true,"name":"x","n":42}'), null);
  assert.match(badReply('/json', 200, 'nope'), /wrong body/);
  assert.equal(badReply('/params/42?q=hello%20world&x=1', 200, 'id=42 q=hello world sid=abc123'), null);
  const li = (n) => '<ul>' + Array.from({ length: n }, (_, i) => `<li>Item &lt;${i + 1}&gt; &amp; co</li>`).join('') + '</ul>';
  assert.equal(badReply('/list', 200, li(50)), null);
  assert.match(badReply('/list', 200, li(49)), /wrong body/);
  assert.equal(badReply('/list1000', 200, li(1000)), null);
});

test('coldOf drops failed starts and coldNote says so', () => {
  const c = coldOf([10, NaN, 30, 20]);
  assert.deepEqual(c, { median: 20, min: 10, n: 3 });
  assert.equal(coldOf([NaN]), null);
  assert.equal(coldNote(15, { a: { n: 15 }, b: { n: 15 } }), 'cold start median of 15');
  assert.match(coldNote(15, { a: { n: 15 }, b: { n: 11 } }), /11 to 15 of 15/);
});

test('redoResets keeps the last try and returns every try', async () => {
  const seq = [{ rps: 9, bad: 3 }, { rps: 5, bad: 2 }, { rps: 7, bad: 0 }];
  let i = 0;
  const { r, tries } = await redoResets(() => seq[i++], (x) => x.bad > 0);
  assert.equal(r.rps, 7);
  assert.deepEqual(tries.map((x) => x.rps), [9, 5, 7]);
  i = 0;
  const all = [{ rps: 1, bad: 1 }, { rps: 99, bad: 1 }, { rps: 2, bad: 1 }, { rps: 3, bad: 1 }];
  const t = await redoResets(() => all[i++], () => true);
  assert.equal(t.tries.length, 3);   // 3 tries at most
  assert.equal(t.r.rps, 2);          // the last, not the best
  i = 0;
  assert.equal((await redoResets(() => all[i++], () => false)).tries.length, 1);
});

test('statTicks reads pgrp and utime+stime', () => {
  const t = '12 (a (b) c) S 1 77 77 0 -1 4194560 1 0 0 0 30 12 0 0 20 0 1 0';
  assert.deepEqual(statTicks(t), { pgrp: 77, ticks: 42 });
});

test('waitIdle waits until the server is under 5% of a core', async () => {
  let t = 0, calls = 0;
  const busy = [100, 100, 100];   // ticks used per 2 s nap: busy, busy, then idle
  const ticks = () => t;
  const nap = async () => { t += busy[calls++] ?? 3; };
  assert.equal(await waitIdle(ticks, { every: 2000, nap }), 6);
  assert.equal(await waitIdle(() => (t += 50), { every: 2000, max: 6000, nap: async () => {} }), -1);
});
