// Runs a Wisp app built for wasm32-unknown-unknown (app.wasm) in any
// JavaScript host. The ABI is described in crates/wisp/src/edge.rs.
const enc = new TextEncoder();
const dec = new TextDecoder();
const failed = { status: 500, headers: [['content-type', 'text/plain; charset=utf-8']], body: enc.encode('Internal Server Error') };

// A first line, `name: value` lines, an empty line, the body.
function head(first, headers) {
  let h = first + '\n';
  for (const [k, v] of headers) h += `${k}: ${v}\n`;
  return enc.encode(h + '\n');
}

function encode(first, headers, body) {
  const h = head(first, headers);
  const out = new Uint8Array(h.length + body.length);
  out.set(h);
  out.set(body, h.length);
  return out;
}

const none = new Uint8Array();
const settled = Promise.resolve();

function decode(bytes) {
  let end = bytes.indexOf(10);
  while (end >= 0 && bytes[end + 1] !== 10) end = bytes.indexOf(10, end + 1);
  if (end < 0) end = bytes.length;
  const text = dec.decode(bytes.subarray(0, end));
  const nl = text.indexOf('\n');
  const headers = [];
  for (let i = nl < 0 ? text.length : nl + 1; i < text.length; ) {
    let e = text.indexOf('\n', i);
    if (e < 0) e = text.length;
    const c = text.indexOf(':', i);
    if (c > i && c < e) headers.push([text.slice(i, c), text.slice(c + 1, e).trim()]);
    i = e + 1;
  }
  return { first: nl < 0 ? text : text.slice(0, nl), headers, body: bytes.slice(end + 2) };
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

// Saved tables (crates/wisp/src/edge_store.rs), kept where WISP_STORE says:
// `d1:BINDING`, `deno-kv[:path]`, or a libSQL server over HTTP (`libsql://`
// or `https://`, with WISP_STORE_TOKEN). Rows are [table, id, json].
const SQL = {
  table: 'create table if not exists wisp_rows (tbl text not null, id integer not null, json text not null, primary key (tbl, id))',
  all: 'select tbl, id, json from wisp_rows',
  put: 'insert or replace into wisp_rows (tbl, id, json) values (?, ?, ?)',
  del: 'delete from wisp_rows where tbl = ? and id = ?',
};
const change = ([t, id, json]) => (json ? [SQL.put, t, id, json] : [SQL.del, t, id]);

function store(env) {
  const spec = env.WISP_STORE;
  if (spec.startsWith('d1:')) {
    const db = env[spec.slice(3)];
    if (!db?.batch) throw new Error(`there is no D1 binding ${spec.slice(3)}`);
    const sql = ([q, ...args]) => db.prepare(q).bind(...args);
    return {
      all: async () => (await db.batch([sql([SQL.table]), sql([SQL.all])]))[1].results.map((r) => [r.tbl, r.id, r.json]),
      write: (rows) => db.batch(rows.map((r) => sql(change(r)))),
    };
  }
  if (spec === 'deno-kv' || spec.startsWith('deno-kv:')) {
    const kv = Deno.openKv(spec.slice(8) || undefined);
    return {
      all: async () => {
        const rows = [];
        for await (const e of (await kv).list({ prefix: ['wisp'] })) rows.push([e.key[1], e.key[2], e.value]);
        return rows;
      },
      // An atomic write takes at most 1000 changes (and 800 KB).
      write: async (rows) => {
        for (let i = 0; i < rows.length; i += 100) {
          const op = (await kv).atomic();
          for (const [t, id, json] of rows.slice(i, i + 100)) json ? op.set(['wisp', t, id], json) : op.delete(['wisp', t, id]);
          if (!(await op.commit()).ok) throw new Error('Deno KV refused the write');
        }
      },
    };
  }
  if (/^(libsql|https?):\/\//.test(spec)) {
    const url = spec.replace(/^libsql:/, 'https:').replace(/\/$/, '') + '/v2/pipeline';
    // One batch of statements, each run only if the one before went well.
    const run = async (stmts) => {
      const arg = (v) => (typeof v === 'number' ? { type: 'integer', value: String(v) } : { type: 'text', value: v });
      const steps = stmts.map(([sql, ...args], i) => ({ stmt: { sql, args: args.map(arg) }, condition: i ? { type: 'ok', step: i - 1 } : undefined }));
      const res = await fetch(url, {
        method: 'POST',
        headers: { authorization: `Bearer ${env.WISP_STORE_TOKEN ?? ''}`, 'content-type': 'application/json' },
        body: JSON.stringify({ requests: [{ type: 'batch', batch: { steps } }, { type: 'close' }] }),
      });
      if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
      const r = (await res.json()).results[0];
      const failed = r.type === 'ok' ? r.response.result.step_errors.find(Boolean) : r.error;
      if (failed) throw new Error(failed.message);
      return r.response.result.step_results.at(-1);
    };
    return {
      all: async () => (await run([[SQL.table], [SQL.all]])).rows.map(([t, id, json]) => [t.value, Number(id.value), json.value]),
      write: (rows) => run([['begin'], ...rows.map(change), ['commit']]),
    };
  }
  throw new Error(`it is ${spec}; use d1:BINDING, deno-kv or a libsql:// address`);
}

// `GET wisp:store` answers every row, a `table\tid\tjson` line each; `POST`
// writes such lines, in one batch (an empty json removes the row).
async function stored(x, env, method, body) {
  try {
    x.store ??= store(env);
    if (method === 'GET') return encode('200', [], enc.encode((await x.store.all()).map((r) => r.join('\t') + '\n').join('')));
    const rows = dec.decode(body).split('\n').filter(Boolean).map((l) => {
      const [a, b] = [l.indexOf('\t'), l.indexOf('\t', l.indexOf('\t') + 1)];
      return [l.slice(0, a), Number(l.slice(a + 1, b)), l.slice(b + 1)];
    });
    await x.store.write(rows);
    return encode('200', [], new Uint8Array());
  } catch (e) {
    return encode('500', [], enc.encode(String(e?.message ?? e)));
  }
}

// `module` is a compiled WebAssembly.Module; `env` the host's variables
// (only strings are passed on). A panic fails its own request with a 500,
// and later requests go to a fresh instance.
export function wisp(module, env = {}) {
  let live = null;
  let ready = null; // the instance that last answered: requests skip the awaits
  let next = 0;

  async function start() {
    // `work`: timers and fetches under way, which `idle` waits out.
    const x = { pending: new Map(), streams: new Map(), retired: false, work: 0, idlers: [] };
    let view; // the memory's bytes, made again only when it has grown
    const mem = () => {
      const b = x.exports.memory.buffer;
      return view?.buffer === b ? view : (view = new Uint8Array(b));
    };
    const copy = (p, n) => mem().slice(p, p + n);
    x.put = (bytes, tail = none) => {
      const n = bytes.length + tail.length;
      const p = x.exports.wisp_buf(n); // may grow memory: view it after
      const m = mem();
      m.set(bytes, p);
      m.set(tail, p + bytes.length);
      return n;
    };
    // A request's head and body into the app's memory, the head encoded in place.
    x.request = (first, headers, body) => {
      let h = first + '\n';
      for (const [k, v] of headers) h += `${k}: ${v}\n`;
      h += '\n';
      const room = h.length * 3; // the most UTF-8 can take
      const p = x.exports.wisp_buf(room + body.length);
      const m = mem();
      const { written } = enc.encodeInto(h, m.subarray(p, p + room));
      m.set(body, p + written);
      return written + body.length;
    };
    x.idle = () => (x.work ? new Promise((r) => x.idlers.push(r)) : settled);
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
        now: () => Date.now() / 1000,
        log: (p, n) => console.error(dec.decode(copy(p, n))),
        reply: (id, p, n) => {
          const done = x.pending.get(id);
          x.pending.delete(id);
          const r = decode(mem().subarray(p, p + n)); // only its body is copied
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
        fetch: (id, p, n) => {
          const r = decode(mem().subarray(p, p + n));
          const [method, url] = r.first.split(' ', 2);
          const answer = url === 'wisp:store' ? stored(x, env, method, r.body) : outbound(r);
          x.later(answer, (b) => x.call(() => x.exports.wisp_fetched(id, x.put(b))));
        },
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
    if (!x.retired) return (ready = x);
    if (live === p) live = null;
    return instance();
  }

  // One request as plain parts: { method, target, peer, headers: [[name, value]], body: Uint8Array }.
  // The answer's body is a Uint8Array, or a ReadableStream for a streamed
  // response. `idle` settles once the timers and fetches the app started
  // (`wisp::spawn`, `wisp::sleep`) are done: pass it to the host's `waitUntil`.
  async function handle({ method, target, peer = '', headers, body }) {
    let x = ready;
    if (!x || x.retired) {
      try {
        x = await instance();
      } catch (e) {
        console.error(e);
        return { ...failed, idle: settled };
      }
    }
    const id = (next = (next + 1) & 0x7fffffff);
    const answer = new Promise((resolve) => x.pending.set(id, resolve));
    // Written into the app's memory as it is: no joined copy first.
    x.call(() => x.exports.wisp_request(id, x.request(`${method} ${target} ${peer}`, headers, body)));
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
    const body = request.body ? new Uint8Array(await request.arrayBuffer()) : none;
    const r = await handle({ method: request.method, target: url.pathname + url.search, peer, headers, body });
    if (r.idle !== settled) ctx?.waitUntil?.(r.idle); // only when work is under way
    const empty = r.status < 200 || r.status === 204 || r.status === 304 || request.method === 'HEAD';
    if (empty && r.body instanceof ReadableStream) r.body.cancel(); // ends the app's stream
    return new Response(empty ? null : r.body, { status: r.status, headers: r.headers });
  }

  return { handle, fetch: serve };
}
