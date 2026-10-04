// A Bun server for a Wisp app: `bun server.mjs` listens on $PORT (3000)
// and $HOST (0.0.0.0).
//
// The server reads and writes raw sockets (Bun.listen) and the app's own HTTP
// parser answers: no Request or Response per request. It is used once a request
// over loopback has been answered as the app answers it; WISP_NODE_HTTP=1, or a
// failed check, serves with Bun.serve instead.
import { wisp } from './bridge.mjs';

const app = wisp(await WebAssembly.compile(await Bun.file(`${import.meta.dir}/app.wasm`).arrayBuffer()), process.env);
let raws = 0;

// A socket's bytes to the app and its answers back. `write` may take only
// part of what it is given: the rest waits in `queue` for `drain`.
const sockets = {
  open(socket) {
    const queue = [];
    let ending = false;
    const flush = () => {
      while (queue.length) {
        const n = socket.write(queue[0]);
        if (n < queue[0].length) return void (queue[0] = queue[0].subarray(Math.max(n, 0)));
        queue.shift();
      }
      if (ending) socket.end();
      return true;
    };
    const io = {
      // A view of the app's memory: copied here.
      write(v) {
        const b = Buffer.from(v);
        const n = queue.length ? 0 : socket.write(b);
        if (n === b.length) return true;
        queue.push(b.subarray(Math.max(n, 0)));
        return false;
      },
      end() {
        ending = true;
        if (!queue.length) socket.end();
      },
      destroy: () => socket.terminate(),
    };
    const c = app.conn(io, socket.remoteAddress ?? '');
    if (!c) return socket.end();
    raws++;
    socket.data = { c, flush };
  },
  data: (socket, bytes) => socket.data?.c.data(bytes),
  drain(socket) {
    if (socket.data?.flush() === true) socket.data.c.drain();
  },
  close: (socket) => socket.data?.c.close(),
  error() {}, // 'close' follows
};

// Whether raw sockets answer: a probe server on loopback, asked as a client would.
async function verified() {
  let probe;
  try {
    probe = Bun.listen({ hostname: '127.0.0.1', port: 0, socket: sockets });
    raws = 0;
    return (await app.check(`http://127.0.0.1:${probe.port}/`)) && raws > 0;
  } catch {
    return false;
  } finally {
    probe?.stop(true);
  }
}

const port = Number(process.env.PORT) || 3000;
const hostname = process.env.HOST || '0.0.0.0';
const forced = process.env.WISP_NODE_HTTP === '1';
const raw = !forced && (await verified());
console.error(`wisp: bun serves with ${raw ? 'raw sockets' : 'Bun.serve'} (${raw ? 'checked' : forced ? 'WISP_NODE_HTTP=1' : 'the check failed'})`);
if (raw) {
  Bun.listen({ hostname, port, socket: sockets });
  console.log(`wisp: listening on http://${hostname}:${port}`);
} else {
  Bun.serve({
    port,
    hostname,
    fetch: (request, server) => app.fetch(request, server.requestIP(request)?.address ?? ''),
  });
  console.log(`wisp: listening on http://${hostname}:${port}`);
}
