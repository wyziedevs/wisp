// Run by tests/fast.rs: node fast.mjs <native url>, in the folder that has bridge.mjs and app.wasm.
// The app's wasm behind the bridge's web `fetch` (what Workers, Deno and the
// edge functions call) against the native server, request by request. One JSON
// line each (the case, whether it was the same, how many times the bridge
// entered the wasm for it); exits 1 when one is not the same, or a path the
// table has went to the wasm, or a path that is not constant did not.
import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const dir = dirname(fileURLToPath(import.meta.url));
const { wisp } = await import(pathToFileURL(join(dir, 'bridge.mjs')).href);
const native = process.argv[2];
// `-`: no native server; the app has hooks, so no path is constant: every request enters the wasm.
const hooks = native === '-';

// Counts the entries of the app's wasm for a request.
let calls = 0;
const instantiate = WebAssembly.instantiate;
WebAssembly.instantiate = async (module, imports) => {
  const instance = await instantiate(module, imports);
  const e = instance.exports;
  return { exports: { ...e, wisp_request_lazy: (...a) => (calls++, e.wisp_request_lazy(...a)) } };
};

const app = wisp(await WebAssembly.compile(readFileSync(join(dir, 'app.wasm'))), { WISP_DEV: 'off' });
const framing = new Set(['content-length', 'date', 'server', 'connection', 'keep-alive', 'transfer-encoding']);
const essence = async (r, headers) => JSON.stringify([r.status, headers ? [...r.headers].filter(([k]) => !framing.has(k)).sort() : 0, await r.text()]);

// The first request starts the instance and takes the slow way.
await (await app.fetch(new Request('http://wisp.test/warm'), '')).text();

if (hooks) {
  for (const p of ['/', '/', '/', '/styled', '/styled']) {
    const before = calls;
    await (await app.fetch(new Request('http://wisp.test' + p, { headers: { accept: 'text/html' } }), '')).text();
    await new Promise((r) => setTimeout(r, 5));
    console.log(JSON.stringify({ case: `GET ${p}`, calls: calls - before }));
    if (calls === before) {
      console.error(`GET ${p} did not enter the wasm, in an app with hooks`);
      process.exit(1);
    }
  }
  process.exit(0);
}

let tag = '';
const cases = [];
const add = (method, path, headers = {}, exact = true) => cases.push({ method, path, headers, exact });
for (const p of ['/about', '/about']) add('GET', p);
add('HEAD', '/about');
for (const inm of ['TAG', '*', 'W/"x", TAG', 'W/TAG', 'W/"nope"', '"nope"', '']) for (const m of ['GET', 'HEAD']) add(m, '/about', { 'if-none-match': inm });
for (const p of ['/about/', '/about/']) add('GET', p);
add('HEAD', '/about/');
add('GET', '/about?x=1', {}, false);
add('GET', '/about?x=1', {}, false);
// A page with a guard (here a rate limit) is the same page, but its guard runs every time.
for (let i = 0; i < 3; i++) add('GET', '/limited', {}, false);
add('GET', '/about', { 'x-wisp-error': '1' }, false);
add('GET', '/nope', {}, false);
add('GET', '/nope', {}, false);
add('GET', '/nope/');
add('GET', '/nope/');
add('GET', '/about', { cookie: 'a=b', accept: 'text/html', 'accept-encoding': 'gzip' });

const seen = new Set();
const bad = [];
for (const c of cases) {
  const headers = { accept: 'text/html', ...Object.fromEntries(Object.entries(c.headers).map(([k, v]) => [k, v.replace('TAG', tag)])) };
  const init = { method: c.method, headers, redirect: 'manual' };
  const before = calls;
  const got = await app.fetch(new Request('http://wisp.test' + c.path, init), '');
  const mine = await essence(got.clone(), c.exact);
  // The probe of a first answer's 304 runs when the call returns.
  await new Promise((r) => setTimeout(r, 5));
  const n = calls - before;
  const want = await essence(await fetch(native + c.path, init), c.exact);
  if (!tag && c.path === '/about' && c.method === 'GET') tag = got.headers.get('etag') ?? '';
  const row = { case: `${c.method} ${c.path} ${JSON.stringify(c.headers)}`, same: mine === want, calls: n };
  console.log(JSON.stringify(row));
  if (mine !== want) bad.push(`${row.case}\n  edge:   ${mine.slice(0, 400)}\n  native: ${want.slice(0, 400)}`);
  // Not constant: the wasm answers. Constant, and seen before: it does not.
  if (!c.exact && n < 1) bad.push(`${row.case}: answered without the wasm`);
  if (c.exact && seen.has(c.path) && n !== 0) bad.push(`${row.case}: entered the wasm ${n} times`);
  if (c.exact) seen.add(c.path);
}
if (bad.length) {
  console.error(bad.join('\n'));
  process.exit(1);
}
