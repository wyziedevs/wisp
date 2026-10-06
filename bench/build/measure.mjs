// Build-time and dev-reload bench, run ON the Linux box (bench/build/README.md).
//   node measure.mjs run <contender> [--runs 3]     every cell of one contender -> results/<contender>.json
//   node measure.mjs step <contender> <cell> <ver>  (internal) one timed build, run under flock by `run`
//   node measure.mjs dev <contender> <runs>         (internal) the dev-reload cells, run under flock by `run`
//   node measure.mjs clean <contender>              removes the installed deps and build output
// Every timed run and every heavy install holds `flock /tmp/wisp-bench.lock`. Inside the lock a run first
// drains (machine under 5% of a core busy), reads /proc/stat, runs, reads it again: steal over 10% of all CPU
// time during the run makes it invalid (retried up to 3 times, then the cell publishes nothing).
import { spawn, spawnSync, execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync, existsSync, rmSync, copyFileSync, mkdirSync, readdirSync, lstatSync, cpSync, openSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import http from 'node:http';
import { parseCpu, stealPct, busyPerSec, validRun, editText, IDLE_BUSY, MAX_TRIES } from './lib.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, '..', '..');
const LOCK = '/tmp/wisp-bench.lock';
const PORT = 3100;
const C = JSON.parse(readFileSync(join(here, 'contenders.json'), 'utf8'));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const cpu = () => parseCpu(readFileSync('/proc/stat', 'utf8'));
const env = { ...process.env, PATH: `/opt/node/bin:/opt/bun/bin:${process.env.HOME}/.cargo/bin:${process.env.PATH}`, NEXT_TELEMETRY_DISABLED: '1', NUXT_TELEMETRY_DISABLED: '1', DO_NOT_TRACK: '1', CI: '1' };
delete env.CARGO_BUILD_JOBS; // the same for every contender: unset, cargo's default (one job per core)
const appDir = (c) => join(repo, C[c].dir);
const sh = (cmd, cwd, log) => spawnSync('sh', ['-c', cmd], { cwd, env, stdio: ['ignore', openSync(log, 'a'), openSync(log, 'a')] }).status;
const logFile = (c, what) => { mkdirSync('/root/bb/logs', { recursive: true }); return `/root/bb/logs/${c}-${what}.log`; };

// Waits until the machine is under IDLE_BUSY jiffies/s busy over a 2 s window. Seconds waited, or -1 after 180 s.
async function drain() {
  const t0 = Date.now();
  for (;;) {
    const a = cpu(); await sleep(2000); const b = cpu();
    if (busyPerSec(a, b, 2) < IDLE_BUSY) return Math.max(0, Math.round((Date.now() - t0) / 1000) - 2);
    if (Date.now() - t0 > 180000) return -1;
  }
}

const origOf = (c, kind) => `/root/bb/orig/${c}/${kind}`;
// The file the edit of `kind` changes, with its original kept beside it. Edit number `ver` = a new version tag.
function edit(c, kind, ver) {
  const f = join(appDir(c), C[c][kind]);
  const keep = origOf(c, kind); // outside the app: a stray copy in src/routes would be a route
  if (!existsSync(keep)) { mkdirSync(dirname(keep), { recursive: true }); copyFileSync(f, keep); }
  const orig = readFileSync(keep, 'utf8');
  const base = (kind === 'logic' && C[c].base) || 'Hello, World!';
  if (!orig.includes(base)) throw new Error(`${f} has no ${base}`);
  writeFileSync(f, editText(orig, base, ver));
  return `${base.replace(/^.*?(Hello, World!)$/, '$1')} v${ver}`;
}
function restore(c) {
  for (const k of ['logic', 'markup']) if (C[c][k] && existsSync(origOf(c, k))) { copyFileSync(origOf(c, k), join(appDir(c), C[c][k])); rmSync(origOf(c, k)); }
}

function dirBytes(p) {
  const s = lstatSync(p);
  if (!s.isDirectory()) return s.size;
  return readdirSync(p).reduce((n, e) => n + dirBytes(join(p, e)), 0);
}

// ---- one timed build, under the lock ----
async function step(c, cell, ver) {
  const k = C[c];
  if (cell === 'cold') for (const p of k.clean) rmSync(join(appDir(c), p), { recursive: true, force: true });
  if (cell === 'warm_edit') edit(c, 'logic', ver);
  if (cell === 'warm_edit_markup') edit(c, 'markup', ver);
  const drain_s = await drain();
  const a = cpu(); const t0 = performance.now();
  const code = sh(k.build, appDir(c), logFile(c, cell));
  const ms = performance.now() - t0; const b = cpu();
  console.log('RESULT ' + JSON.stringify({ ms, steal: stealPct(a, b), drain_s, code }));
}

// ---- dev reload, under the lock: start the dev server, edit, poll over HTTP until the new text is served ----
const get = (path) => new Promise((res) => {
  const r = http.get({ host: '127.0.0.1', port: PORT, path, agent: false, timeout: 3000 }, (m) => { let s = ''; m.on('data', (d) => (s += d)); m.on('end', () => res(s)); });
  r.on('error', () => res('')); r.on('timeout', () => { r.destroy(); res(''); });
});
async function until(path, needle, limit) {
  const t0 = performance.now();
  for (;;) {
    if ((await get(path)).includes(needle)) return performance.now() - t0;
    if (performance.now() - t0 > limit) return null;
    await sleep(5);
  }
}
async function dev(c, runs) {
  const k = C[c];
  const log = logFile(c, 'dev');
  const child = spawn('sh', ['-c', k.dev.cmd.replace('{port}', PORT)], { cwd: appDir(c), env, detached: true, stdio: ['ignore', openSync(log, 'w'), openSync(log, 'w')] });
  const out = {};
  try {
    if ((await until('/plaintext', 'Hello, World!', k.dev.ready * 1000)) == null) throw new Error('dev server never answered');
    if (k.markup) await until('/hello', 'Hello, World!', 120000); // first request compiles the route: not timed
    for (const [cell, kind, path] of [['dev_logic', 'logic', '/plaintext'], ['dev_markup', 'markup', '/hello']]) {
      if (!k[kind]) continue;
      const res = { ms: [], steal: [], drain_s: [], invalid: [], tries: 0 };
      for (let i = 0; i < runs; i++) {
        for (let t = 0; t < MAX_TRIES; t++) {
          res.tries++;
          const drain_s = await drain();
          const a = cpu(); const ver = Date.now();
          const t0 = performance.now(); const needle = edit(c, kind, ver);
          const ms = await until(path, needle, 60000); const b = cpu();
          const run = { ms, steal: stealPct(a, b), drain_s };
          if (ms != null && validRun(run)) { res.ms.push(ms); res.steal.push(run.steal); res.drain_s.push(drain_s); break; }
          res.invalid.push(run);
          await sleep(2000);
        }
      }
      out[cell] = res.ms.length === runs ? res : { na: `invalid: ${runs} valid runs not reached in ${MAX_TRIES} tries each`, invalid: res.invalid };
    }
  } catch (e) {
    out.error = String(e);
  } finally {
    try { process.kill(-child.pid, 'SIGTERM'); } catch {}
    await sleep(1500);
    try { process.kill(-child.pid, 'SIGKILL'); } catch {}
    restore(c);
  }
  console.log('RESULT ' + JSON.stringify(out));
}

// ---- orchestration (not itself timed) ----
const locked = (...args) => {
  const r = spawnSync('flock', [LOCK, process.execPath, join(here, 'measure.mjs'), ...args], { env, encoding: 'utf8', maxBuffer: 1 << 26, stdio: ['ignore', 'pipe', 'inherit'] });
  const line = (r.stdout || '').split('\n').filter((l) => l.startsWith('RESULT ')).pop();
  if (!line) throw new Error(`no result from ${args.join(' ')}: ${r.stdout}`);
  return JSON.parse(line.slice(7));
};
const toolVersions = () => {
  const v = (cmd, a) => { try { return execFileSync(cmd, a, { env, encoding: 'utf8' }).trim().split('\n')[0]; } catch { return null; } };
  return { nproc: Number(v('nproc', [])), cargo_build_jobs: 'unset for every contender (cargo default, one job per core)', node: v('node', ['-v']), bun: v('bun', ['-v']), rustc: v('rustc', ['-V']), cargo: v('cargo', ['-V']), deno: v('deno', ['-V']), kernel: v('uname', ['-r']),
    ip_local_port_range: readFileSync('/proc/sys/net/ipv4/ip_local_port_range', 'utf8').trim().replace(/\s+/g, ' '), lock: LOCK, poll: 'node http GET, 5 ms between replies' };
};
const npmVersions = (c) => { try { return JSON.parse(execFileSync('npm', ['ls', '--depth=0', '--json'], { cwd: appDir(c), env, encoding: 'utf8' })).dependencies; } catch { return undefined; } };

function runCell(c, cell, runs) {
  const res = { ms: [], steal: [], drain_s: [], invalid: [], tries: 0 };
  for (let i = 0; i < runs; i++) {
    for (let t = 0; t < MAX_TRIES; t++) {
      res.tries++;
      const r = locked('step', c, cell, String(Date.now()));
      if (r.code !== 0) return { na: `build failed (exit ${r.code}), see ${logFile(c, cell)}` };
      if (validRun(r)) { res.ms.push(r.ms); res.steal.push(r.steal); res.drain_s.push(r.drain_s); break; }
      res.invalid.push({ ms: r.ms, steal: r.steal, drain_s: r.drain_s });
    }
  }
  return res.ms.length === runs ? res : { na: `invalid: ${runs} valid runs not reached in ${MAX_TRIES} tries each`, invalid: res.invalid };
}

async function main() {
  const [mode, c, ...rest] = process.argv.slice(2);
  if (mode === 'step') return step(c, rest[0], Number(rest[1]));
  if (mode === 'dev') return dev(c, Number(rest[0]));
  if (mode === 'clean') { for (const p of ['target', 'node_modules', '.svelte-kit', 'build', '.next', '.nuxt', '.output']) rmSync(join(appDir(c), p), { recursive: true, force: true }); return; }
  if (mode !== 'run') throw new Error('usage: measure.mjs run <contender> [--runs 3]');
  const runs = Number(rest[rest.indexOf('--runs') + 1]) || 3;
  const k = C[c];
  const out = join(here, 'results', `${c}.json`);
  mkdirSync(dirname(out), { recursive: true });
  const r = { contender: c, when: new Date().toISOString(), runs, host: toolVersions(), cells: {} };
  const save = () => writeFileSync(out, JSON.stringify(r, null, 1) + '\n');
  restore(c);
  if (k.overlay) cpSync(join(here, k.overlay), appDir(c), { recursive: true });
  const t = Date.now();
  spawnSync('flock', [LOCK, 'sh', '-c', k.install], { cwd: appDir(c), env, stdio: ['ignore', openSync(logFile(c, 'install'), 'a'), 'inherit'] }); // downloads: not measured
  r.deps = k.build?.startsWith('cargo') ? readFileSync(join(appDir(c), 'Cargo.lock'), 'utf8').split('[[package]]').length - 1 + ' locked crates' : npmVersions(c);
  console.error(`${c}: installed in ${Math.round((Date.now() - t) / 1000)} s`);
  if (k.noBuild) for (const cell of ['cold', 'warm_noop', 'warm_edit', 'warm_edit_markup']) r.cells[cell] = { na: k.noBuild };
  else {
    r.cells.cold = runCell(c, 'cold', runs); save();
    r.cells.warm_noop = runCell(c, 'warm_noop', runs); save();
    r.cells.warm_edit = runCell(c, 'warm_edit', runs); save();
    if (k.markup) r.cells.warm_edit_markup = runCell(c, 'warm_edit_markup', runs);
    restore(c);
  }
  try { r.artifact = { bytes: dirBytes(join(appDir(c), k.artifact.path)), what: k.artifact.what }; } catch { /* build failed */ }
  if (k.noDev) { r.cells.dev_logic = { na: k.noDev }; if (k.markup) r.cells.dev_markup = { na: k.noDev }; }
  else {
    const d = locked('dev', c, String(runs));
    if (d.error) r.cells.dev_logic = { na: d.error }; else Object.assign(r.cells, d);
    if (k.dev.note) r.dev_note = k.dev.note;
  }
  save();
  console.error(`${c}: done`, JSON.stringify(Object.fromEntries(Object.entries(r.cells).map(([n, x]) => [n, x.ms ? Math.round(x.ms.sort((a, b) => a - b)[x.ms.length >> 1]) : x.na]))));
}
await main();
