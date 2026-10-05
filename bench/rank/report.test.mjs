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
  const cell = (rps, bad = 0) => ({ rps, p99: 1, runs: [rps], bad, ...(bad && { failed: true }) });
  writeFileSync(join(d, 'node.json'), JSON.stringify({
    host: 'node', when: '2026-01-01T00:00:00Z', secs: '1', runs: 1, conns: '64', colds: 15, valid: true,
    cells: { 'wisp raw /': cell(900), 'hono /': cell(null, 5), 'fastify /': cell(500) },
    cold: { 'wisp raw': { median: 10, min: 9, n: 15 }, hono: { median: 20, min: 18, n: 11 }, fastify: { median: 30, min: 29, n: 15 } },
    rss: {}, failed: {},
  }));
  const out = execFileSync(process.execPath, [join(dirname(fileURLToPath(import.meta.url)), 'report.mjs'), d]).toString();
  assert.match(out, /\| hono \| Failed \|/);
  assert.match(out, /\*\*900 \(#1\)\*\*/);
  assert.match(out, /fastify \| 500 \(#2\)/);
  assert.match(out, /11 to 15 of 15/);
});
