// Deno Deploy entry: `deno run --allow-read --allow-env --allow-net main.ts`
// locally, `deployctl deploy --entrypoint main.ts` to deploy.
//
// Run locally, the server reads and writes raw sockets (Deno.listen) and the
// app's own HTTP parser answers: no Request or Response per request. It is
// used once a request over loopback has been answered as the app answers it;
// WISP_NODE_HTTP=1, a failed check, or Deploy (which has no sockets) serves
// with Deno.serve instead.
import { wisp } from './bridge.mjs';

const wasm = await Deno.readFile(new URL('./app.wasm', import.meta.url));
const app = wisp(await WebAssembly.compile(wasm), Deno.env.toObject());

const HIGH = 1 << 16;
const IDLE = 60_000;
let raws = 0;

// A socket's bytes to the app and its answers back, until it closes.
async function accept(conn: Deno.TcpConn) {
  let queued = 0;
  let behind = false;
  let wake: (() => void) | null = null;
  let tail: Promise<unknown> = Promise.resolve();
  let timer = 0;
  const arm = () => (clearTimeout(timer), (timer = setTimeout(() => conn.close(), IDLE)));
  const done = (n: number) => {
    queued -= n;
    if (queued < HIGH) {
      wake?.();
      if (behind) (behind = false), c?.drain();
    }
  };
  const c = app.conn(
    {
      // A view of the app's memory: copied here.
      write(v: Uint8Array) {
        const b = v.slice();
        queued += b.length;
        tail = tail.then(async () => {
          for (let o = 0; o < b.length; ) o += await conn.write(b.subarray(o));
          done(b.length);
        }).catch(() => conn.close());
        behind = queued >= HIGH;
        return !behind;
      },
      end() {
        tail = tail.then(() => conn.closeWrite()).then(() => setTimeout(() => { try { conn.close(); } catch { /* closed */ } }, 5000), () => {});
      },
      destroy: () => { try { conn.close(); } catch { /* closed */ } },
    },
    (conn.remoteAddr as Deno.NetAddr).hostname,
  );
  if (!c) return conn.close();
  raws++;
  const buf = new Uint8Array(HIGH);
  try {
    arm();
    for (;;) {
      while (queued >= HIGH) await new Promise<void>((r) => (wake = r));
      const n = await conn.read(buf);
      if (n === null) break;
      arm();
      c.data(buf.subarray(0, n)); // copied into the app's memory before it returns
    }
  } catch { /* reset or closed */ }
  clearTimeout(timer);
  c.close();
  try { conn.close(); } catch { /* closed */ }
}

async function serve(listener: Deno.Listener, open?: Set<Deno.Conn>) {
  try {
    for await (const conn of listener) {
      open?.add(conn);
      accept(conn as Deno.TcpConn).finally(() => open?.delete(conn));
    }
  } catch { /* the listener closed */ }
}

// Whether raw sockets answer: a probe server on loopback, asked as a client would.
async function verified() {
  const open = new Set<Deno.Conn>();
  let probe: Deno.Listener | undefined;
  try {
    probe = Deno.listen({ hostname: '127.0.0.1', port: 0 });
    serve(probe, open);
    raws = 0;
    return (await app.check(`http://127.0.0.1:${(probe.addr as Deno.NetAddr).port}/`)) && raws > 0;
  } catch {
    return false;
  } finally {
    open.forEach((c) => { try { c.close(); } catch { /* closed */ } });
    probe?.close();
  }
}

const deploy = Deno.env.get('DENO_DEPLOYMENT_ID') !== undefined;
const forced = Deno.env.get('WISP_NODE_HTTP') === '1';
const raw = !deploy && !forced && (await verified());
if (!deploy) console.error(`wisp: deno serves with ${raw ? 'raw sockets' : 'Deno.serve'} (${raw ? 'checked' : forced ? 'WISP_NODE_HTTP=1' : 'the check failed'})`);
if (raw) {
  const port = Number(Deno.env.get('PORT')) || 8000;
  const hostname = Deno.env.get('HOST') || '0.0.0.0';
  const listener = Deno.listen({ hostname, port });
  console.log(`wisp: listening on http://${hostname}:${port}`);
  await serve(listener);
} else {
  Deno.serve((request, info) => app.fetch(request, (info.remoteAddr as Deno.NetAddr).hostname));
}
