// Drives bridge.js with a fake instance, which answers each request with the
// head and body its first request line asks for: `GET /<status>` answers that
// status with a text body of "x" (an empty one for a status that has none).
import { wisp } from '../../src/targets/bridge.js';

const enc = new TextEncoder();
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
      wisp_env() {},
      main() {},
      wisp_current: () => 0xffffffff,
      wisp_poll() {},
      wisp_request(id, len) {
        target = new TextDecoder().decode(bytes().slice(0, len)).split(' ')[1];
        answer(id);
      },
      wisp_request_lazy(id, len) {
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
if (bad.length) {
  console.error(bad.join('\n'));
  process.exit(1);
}
