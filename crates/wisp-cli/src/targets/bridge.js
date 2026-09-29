// Runs a Wisp app built for wasm32-unknown-unknown (app.wasm) in any
// JavaScript host. The ABI is described in crates/wisp/src/edge.rs.
const enc = new TextEncoder();
const dec = new TextDecoder();
const failed = { status: 500, headers: [['content-type', 'text/plain; charset=utf-8']], body: enc.encode('Internal Server Error') };

// A first line, `name: value` lines, an empty line, the body.
function encode(first, headers, body) {
  let head = first + '\n';
  for (const [k, v] of headers) head += `${k}: ${v}\n`;
  const h = enc.encode(head + '\n');
  const out = new Uint8Array(h.length + body.length);
  out.set(h);
  out.set(body, h.length);
  return out;
}

function decode(bytes) {
  let end = 0;
  while (end + 1 < bytes.length && !(bytes[end] === 10 && bytes[end + 1] === 10)) end++;
  const [first, ...lines] = dec.decode(bytes.subarray(0, end)).split('\n');
  const headers = lines.filter((l) => l.indexOf(':') > 0).map((l) => [l.slice(0, l.indexOf(':')), l.slice(l.indexOf(':') + 1).trim()]);
  return { first, headers, body: bytes.slice(end + 2) };
}

// `wisp::edge::fetch`: status 0 is a request that got no answer.
async function outbound({ first, headers, body }) {
  try {
    const at = first.indexOf(' ');
    const res = await fetch(first.slice(at + 1), { method: first.slice(0, at), headers, body: body.length ? body : undefined });
    return encode(String(res.status), [...res.headers], new Uint8Array(await res.arrayBuffer()));
  } catch (e) {
    return encode('0', [], enc.encode(String(e)));
  }
}

// `module` is a compiled WebAssembly.Module; `env` the host's variables
// (only strings are passed on). A panic fails its own request with a 500,
// and later requests go to a fresh instance.
export function wisp(module, env = {}) {
  let live = null;
  let next = 0;

  async function start() {
    // `work`: timers and fetches under way, which `idle` waits out.
    const x = { pending: new Map(), streams: new Map(), retired: false, work: 0, idlers: [] };
    const mem = () => new Uint8Array(x.exports.memory.buffer);
    const copy = (p, n) => mem().slice(p, p + n);
    x.put = (bytes) => {
      const p = x.exports.wisp_buf(bytes.length); // may grow memory: view it after
      mem().set(bytes, p);
      return bytes.length;
    };
    x.idle = () => (x.work ? new Promise((r) => x.idlers.push(r)) : Promise.resolve());
    // Runs `f` once `promise` settles, counted as work until then.
    x.later = (promise, f) => {
      x.work++;
      promise.then(f).finally(() => --x.work || x.idlers.splice(0).forEach((r) => r()));
    };
    x.call = (f) => {
      try {
        f();
      } catch (e) {
        console.error(`wisp: the app failed (${e}); a new instance takes the next requests`);
        x.retired = true;
        const id = x.exports.wisp_current();
        const fail = (id) => {
          x.pending.get(id)?.(null);
          x.pending.delete(id);
          x.streams.get(id)?.error(e);
          x.streams.delete(id);
        };
        if (id !== 0xffffffff) {
          // One task trapped (a request's, or a `wisp::spawn`'s): the others go on.
          fail(id);
          x.call(() => x.exports.wisp_poll());
        } else {
          [...x.pending.keys(), ...x.streams.keys()].forEach(fail);
        }
      }
    };
    const imports = {
      wisp: {
        random: (p, n) => void crypto.getRandomValues(mem().subarray(p, p + n)),
        log: (p, n) => console.error(dec.decode(copy(p, n))),
        reply: (id, p, n) => {
          const done = x.pending.get(id);
          x.pending.delete(id);
          const r = decode(copy(p, n));
          if (r.first.endsWith(' stream')) {
            r.body = new ReadableStream({
              start: (c) => void x.streams.set(id, c),
              // Deferred: the stream may pull from inside `chunk`, while the app runs.
              pull: () => queueMicrotask(() => x.call(() => x.exports.wisp_pull(id))),
              // The client left: dropping the request's task fails the app's sender.
              cancel: () => x.streams.delete(id) && x.call(() => x.exports.wisp_cancel(id)),
            });
          }
          done?.(r);
        },
        // 0 when the client is behind: the app waits for `wisp_pull`.
        chunk: (id, p, n) => {
          const c = x.streams.get(id);
          if (n && c) {
            c.enqueue(copy(p, n));
            return c.desiredSize > 0 ? 1 : 0;
          }
          x.streams.delete(id);
          c?.close();
          return 1;
        },
        fetch: (id, p, n) => x.later(outbound(decode(copy(p, n))), (b) => x.call(() => x.exports.wisp_fetched(id, x.put(b)))),
        timer: (id, ms) => x.later(new Promise((r) => setTimeout(r, ms)), () => x.call(() => x.exports.wisp_timer(id))),
      },
    };
    x.exports = (await WebAssembly.instantiate(module, imports)).exports;
    const vars = Object.entries(env).filter(([, v]) => typeof v === 'string');
    const len = x.put(enc.encode(vars.map(([k, v]) => `${k}=${v}\0`).join('')));
    x.call(() => x.exports.wisp_env(len));
    x.call(() => x.exports.main(0, 0));
    if (x.retired) throw new Error('wisp: the app failed to start');
    return x;
  }

  // The instance new requests go to: after a panic, a fresh one.
  async function instance() {
    if (!live) {
      const p = (live = start());
      p.catch(() => live === p && (live = null));
    }
    const p = live;
    const x = await p;
    if (!x.retired) return x;
    if (live === p) live = null;
    return instance();
  }

  // One request as plain parts: { method, target, peer, headers: [[name, value]], body: Uint8Array }.
  // The answer's body is a Uint8Array, or a ReadableStream for a streamed
  // response. `idle` settles once the timers and fetches the app started
  // (`wisp::spawn`, `wisp::sleep`) are done: pass it to the host's `waitUntil`.
  async function handle({ method, target, peer = '', headers, body }) {
    let x;
    try {
      x = await instance();
    } catch (e) {
      console.error(e);
      return { ...failed, idle: Promise.resolve() };
    }
    const id = (next = (next + 1) & 0x7fffffff);
    const answer = new Promise((resolve) => x.pending.set(id, resolve));
    const bytes = encode(`${method} ${target} ${peer}`, headers, body);
    x.call(() => x.exports.wisp_request(id, x.put(bytes)));
    const r = await answer;
    const idle = x.idle();
    return r ? { status: parseInt(r.first) || 500, headers: r.headers, body: r.body, idle } : { ...failed, idle };
  }

  // A web `Request` to a web `Response`: Workers, Deno, Netlify. `ctx` is
  // the host's context, whose `waitUntil` keeps background work alive.
  async function serve(request, peer = '', ctx) {
    const url = new URL(request.url);
    const headers = [...request.headers];
    if (!request.headers.has('host')) headers.push(['host', url.host]);
    const body = new Uint8Array(await request.arrayBuffer());
    const r = await handle({ method: request.method, target: url.pathname + url.search, peer, headers, body });
    ctx?.waitUntil?.(r.idle);
    const empty = r.status < 200 || r.status === 204 || r.status === 304 || request.method === 'HEAD';
    if (empty && r.body instanceof ReadableStream) r.body.cancel(); // ends the app's stream
    const h = new Headers();
    for (const [k, v] of r.headers) h.append(k, v);
    return new Response(empty ? null : r.body, { status: r.status, headers: h });
  }

  return { handle, fetch: serve };
}
