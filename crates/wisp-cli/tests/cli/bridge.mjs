// Drives bridge.js with a fake instance, which answers each request with the
// head and body its first request line asks for: `GET /<status>` answers that
// status with a text body of "x" (an empty one for a status that has none).
import { put, wisp } from '../../src/targets/bridge.js';

const enc = new TextEncoder();
const targets = [];
const envs = []; // what each instance was given, in order
globalThis.WebAssembly.instantiate = async (_, imports) => {
  const memory = new WebAssembly.Memory({ initial: 1 });
  const bytes = () => new Uint8Array(memory.buffer);
  let target = '';
  const answer = (id, lazy) => {
    const status = Number(target.slice(1)) || 200;
    const head = enc.encode(`${status}\ncontent-type: text/plain; charset=utf-8\nset-cookie: a=1\nset-cookie: b=2\n`);
    bytes().set(head, 4096);
    const body = enc.encode(status === 205 || status === 204 ? '' : 'x');
    bytes().set(body, 8192);
    imports.wisp.reply(id, 4096, head.length, 0xffffffff, 8192, body.length);
  };
  return {
    exports: {
      memory,
      wisp_buf: () => 0,
      wisp_env(len) {
        envs.push(new TextDecoder().decode(bytes().slice(0, len)));
      },
      main() {},
      wisp_current: () => 0xffffffff,
      wisp_poll() {},
      wisp_request(id, len) {
        targets.push(new TextDecoder().decode(bytes().slice(0, len)).split(' ')[1]);
        target = new TextDecoder().decode(bytes().slice(0, len)).split(' ')[1];
        answer(id);
      },
      wisp_request_lazy(id, len) {
        targets.push(new TextDecoder().decode(bytes().slice(0, len)).split(' ')[1]);
        target = new TextDecoder().decode(bytes().slice(0, len)).split(' ')[1];
        answer(id);
      },
    },
  };
};

const app = wisp({}, {});
await app.up();
const bad = [];
for (const status of [200, 204, 205, 304]) {
  for (const body of [false, true]) {
    try {
      const init = body ? { method: 'POST', body: 'in' } : {};
      const res = await app.fetch(new Request(`https://wisp.test/${status}`, init), '');
      if (res.status !== status) bad.push(`${status}: got ${res.status}`);
      const cookies = res.headers.getSetCookie();
      if (cookies.length !== 2) bad.push(`${status}: ${cookies.length} set-cookie headers`);
    } catch (e) {
      bad.push(`${status}${body ? ' with a body' : ''}: ${e.message}`);
    }
  }
}
// worker.js warms a throwaway instance at load, which global scope allows:
// told `WISP_WARM_UP=1` (the runtime then runs no app code), with no random
// number asked for.
envs.length = 0;
targets.length = 0;
const random = crypto.getRandomValues;
crypto.getRandomValues = () => bad.push('the warm-up asked for random values');
await import('./worker.mjs');
await new Promise((r) => setTimeout(r, 50));
crypto.getRandomValues = random;
if (envs.length !== 1 || !envs[0].split('\0').includes('WISP_WARM_UP=1')) bad.push(`warm-up env: ${JSON.stringify(envs)}`);
if (targets.join() !== '/') bad.push(`warm-up requests: ${targets}`);
// A host's socket that takes only 1000 and 3000 to 4999 (Deno's): a close
// for a message too big is 4009, never a normal 1000.
const closed = [];
const deno = { send() {}, close(code, reason) {
  if (code !== 1000 && (code < 3000 || code > 4999)) throw new Error('InvalidAccessError');
  closed.push(`${code} ${reason ?? ''}`.trim());
} };
put(deno, 8, new Uint8Array([0x03, 0xf1]));
put(deno, 8, new Uint8Array([0x03, 0xe8]));
put(deno, 8, new Uint8Array([0x0f, 0xa0, 0x78]));
if (closed.join() !== '4009,1000,4000 x') bad.push(`Deno closes: ${closed}`);
if (bad.length) {
  console.error(bad.join('\n'));
  process.exit(1);
}
