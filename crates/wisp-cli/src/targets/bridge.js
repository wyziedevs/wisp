// Runs a Wisp app built for wasm32-unknown-unknown (app.wasm) in any
// JavaScript host. The ABI is described in crates/wisp/src/edge.rs.
const enc = new TextEncoder();
const dec = new TextDecoder();
// Exact: invalid UTF-8 throws and a leading BOM is kept, so the text encodes back to the same bytes.
const utf8 = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true });
// The same without `fatal`: Deno's fatal decoder takes 12x as long on 14 KB
// (6.7 us, against 0.55; bench/rank /json-big), Bun's and Node's the same.
const loose = new TextDecoder('utf-8', { ignoreBOM: true });
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
const failedHead = { status: 500, headers: ['content-type', 'text/plain; charset=utf-8'], stream: false };
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

// Response heads seen so far, by their text (and, in an instance, by the
// number the app gave them): a route answers with the same head each time. { status, headers: [name, value, ...], stream }.
const heads = new Map();
function same(a, b) {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}
function parsed(text) {
  let h = heads.get(text);
  if (h) return h;
  const nl = text.indexOf('\n');
  const first = text.slice(0, nl < 0 ? text.length : nl);
  h = { status: parseInt(text) || 500, headers: [], stream: first.endsWith(' stream'), same: first.endsWith(' const') };
  for (let i = nl < 0 ? text.length : nl + 1; i < text.length; ) {
    let e = text.indexOf('\n', i);
    if (e < 0) e = text.length;
    const c = text.indexOf(':', i);
    if (c > i && c < e) h.headers.push(text.slice(i, c), text.slice(c + 1, e).trim());
    i = e + 1;
  }
  for (let i = 0; i < h.headers.length; i += 2) if (h.headers[i] === 'content-type') h.text = !h.stream && /text|json|javascript|xml/.test(h.headers[i + 1]);
  // A 200 whose only header is plain UTF-8 text: what `new Response(string)` sends by itself, and an init costs workerd 2.5 us (bench/edge).
  h.plain = h.status === 200 && h.text && h.headers.length === 2 && h.headers[1] === 'text/plain; charset=utf-8';
  if (heads.size > 256) heads.clear();
  heads.set(text, h);
  return h;
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

// The status `GET url` gets, by `fetch` (the default client of `check`).
async function fetched(url) {
  const got = await fetch(url, { redirect: 'manual', signal: AbortSignal.timeout(5000) });
  await got.arrayBuffer();
  return got.status;
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

const CONN = 1 << 30;

// The answer to a `serve` request `c`: a Response, handed to the waiting
// Promise or kept for `serve` to return. Response's constructor copies the
// body, so the app's memory view is safe to pass.
function webSink(c, h, body) {
  if (h.same && c.fast) keep(c, h, body);
  if (c.quiet) return;
  if (h.status === 101) {
    // The app upgraded: the host's own socket takes the request over.
    const r = c.up?.(c) ?? new Response('WebSockets are not available on this host', { status: 501 });
    return c.resolve ? c.resolve(r) : (c.res = r);
  }
  const empty = bare(h.status, c.empty, body);
  const text = !empty && h.text ? asText(body) : body;
  const r = h.plain && !empty && typeof text === 'string' ? new Response(text) : new Response(empty ? null : text, (h.init ??= init(h)));
  if (c.resolve) c.resolve(r);
  else c.res = r;
}
// Whether a reply has no body (1xx, 204, 205, 304, or a HEAD's); then the
// app's stream, if it sends one, is cancelled to end it.
function bare(status, head, body) {
  const empty = head || status < 200 || status === 204 || status === 205 || status === 304;
  if (empty && body instanceof ReadableStream) body.cancel();
  return empty;
}
// A text body as a string: workerd makes a Response from a string in about
// half the time it takes to copy the bytes out of the app's memory (bench/edge).
// Deno's decoder is a call into Rust, 0.9 us however short the text: a short
// ASCII body is read here in 0.1 to 0.4 us.
const ascii = typeof Deno === 'object' ? 64 : 0;
function asText(body) {
  if (body.length <= ascii) {
    let i = 0;
    while (i < body.length && body[i] < 128) i++;
    if (i === body.length) return String.fromCharCode.apply(null, body);
  }
  // U+FFFD marks a byte that was not UTF-8, or one the text has itself: the exact decoder tells.
  const s = loose.decode(body);
  if (!s.includes('�')) return s;
  try {
    return utf8.decode(body);
  } catch {
    return body; // not UTF-8 after all
  }
}
// A reply the app sent as `const` (`wisp::edge::constant`) is the same for
// every request for its path that has no query, sends no `x-wisp-error` and
// answers `if-none-match` by the ETag, whatever else it sends: a baked page, a
// trailing-slash redirect. `serve` keeps the first, and its 304 (a probe of
// the app, so the head is the app's own), and answers the path from them.
function keep(c, h, body) {
  if (c.entry) {
    if (h.status === 304) c.entry.nm = h.init ??= init(h);
    return;
  }
  if (c.fast.has(c.path) || c.fast.size >= 128 || body.length > 1 << 18) return;
  let etag = null;
  for (let i = 0; i < h.headers.length; i += 2) if (h.headers[i] === 'etag') etag = h.headers[i + 1];
  const entry = { init: (h.init ??= init(h)), body: body.slice(), etag, nm: null };
  c.fast.set(c.path, entry);
  if (etag) queueMicrotask(() => c.probe(entry));
}
// Whether an `if-none-match` header names `tag`: `*`, or a list that has it, weakly.
function names(header, tag) {
  return header.split(',').some((t) => {
    t = t.trim();
    while (t.startsWith('W/')) t = t.slice(2);
    return t === '*' || t === tag;
  });
}
// What `new Response` takes for a head, made once: workerd copies a
// `Headers` into the Response quicker than it reads a plain object's names;
// Deno reads a plain object in half the time (110 ns against 196), so it
// gets one when no name repeats.
function init({ status, headers: flat }) {
  if (typeof Deno === 'object') {
    const o = {};
    let i = 0;
    while (i < flat.length && !Object.hasOwn(o, flat[i])) o[flat[i]] = flat[i + 1], (i += 2);
    if (i >= flat.length) return { status, headers: o };
  }
  const headers = new Headers();
  for (let i = 0; i < flat.length; i += 2) headers.append(flat[i], flat[i + 1]);
  return { status, headers };
}

// A message for a WebSocket the host made: `op` 1 text, 2 binary, 8 a close
// (a code and a reason). A host that will not send a protocol code (Deno's
// takes only 1000 and 3000 to 4999) gets it as 4000 and up, 1009 as 4009,
// with the reason: a failure never reads as a normal close.
export function put(ws, op, bytes) {
  try {
    if (op === 1) ws.send(dec.decode(bytes));
    else if (op === 2) ws.send(bytes);
    else if (op === 8) {
      const code = bytes.length > 1 ? (bytes[0] << 8) | bytes[1] : 1000;
      const reason = dec.decode(bytes.subarray(2));
      try {
        ws.close(code, reason);
      } catch {
        ws.close(code > 1000 && code < 1016 ? code + 3000 : 1000, reason);
      }
    }
  } catch {} // closed already
}

// `module` is a compiled WebAssembly.Module; `env` the host's variables
// (only strings are passed on). `accept(request)`, for a host that holds
// WebSockets in a fetch handler, makes one: { ws, response }, `ws` a
// WebSocket (send, close, addEventListener 'message' 'close' 'error' and
// 'open', readyState) and `response` what the handler returns (Workers'
// `new Response(null, { status: 101, webSocket })`, Deno.upgradeWebSocket's).
// Without it the app answers a WebSocket request 501. A panic fails its own request with a 500,
// and later requests go to a fresh instance.
//
// `sink(ctx, head, body)` is the answer to `direct`: a head `{ status,
// headers: [name, value, ...] }` (shared: read it, never change it) and a
// body that is a view of the app's memory, good only until the sink returns,
// or a ReadableStream.
export function wisp(module, env = {}, sink, accept) {
  let live = null;
  let ready = null; // the instance that last answered: requests skip the awaits
  let next = 0;
  const seen = new Map();
  const sent = new Map(); // path -> { method, peer, host, bytes }: a request's text, for `serve`
  const fast = new Map(); // path -> what `keep` made of its `const` reply
  sink ??= webSink;

  async function start() {
    // `work`: timers and fetches under way, which `idle` waits out.
    // `cur`: the request whose call into the app is under way (`curId`, -1
    // for none), kept out of `pending` until it is left unanswered.
    const x = { pending: new Map(), cur: null, curId: -1, heads: [], streams: new Map(), conns: new Map(), socks: new Map(), retired: false, work: 0, idlers: [] };
    // The memory's bytes, made again only when it has grown: growing detaches
    // the old buffer, which empties its view (reading `memory.buffer` each
    // time is a call into the engine).
    let view = none;
    const mem = () => (view.length ? view : (view = new Uint8Array(x.exports.memory.buffer)));
    const copy = (p, n) => mem().slice(p, p + n);
    // What waits for request `id`'s answer, and its end.
    x.of = (id) => (id === x.curId ? x.cur : x.pending.get(id));
    x.done = (id) => (id === x.curId ? (x.curId = -1) : x.pending.delete(id));
    // Room for `n` bytes the app will read: its buffer stays put until more is asked.
    let ptr = 0;
    let cap = 0;
    const room = (n) => {
      if (n > cap) {
        ptr = x.exports.wisp_buf(n); // may grow memory: view it after
        cap = n;
      }
      return ptr;
    };
    x.put = (bytes, tail = none) => {
      const n = bytes.length + tail.length;
      const p = room(n);
      const m = mem();
      m.set(bytes, p);
      m.set(tail, p + bytes.length);
      return n;
    };
    // A request's head and body into the app's memory, the head encoded in place.
    x.request = (h, body) => {
      const span = h.length * 3; // the most UTF-8 can take
      const p = room(span + body.length);
      const m = mem();
      const { written } = enc.encodeInto(h, m.subarray(p, p + span));
      m.set(body, p + written);
      return written + body.length;
    };
    x.stream = (id) =>
      new ReadableStream({
        start: (c) => void x.streams.set(id, c),
        // Deferred: the stream may pull from inside `chunk`, while the app runs.
        pull: () => queueMicrotask(() => x.call(() => x.exports.wisp_pull(id))),
        // The client left: dropping the request's task fails the app's sender.
        cancel: () => x.streams.delete(id) && x.call(() => x.exports.wisp_cancel(id)),
      });
    // Request bytes already encoded, as they are into the app's memory.
    x.write = (bytes) => {
      const p = room(bytes.length);
      mem().set(bytes, p);
      return bytes.length;
    };
    // `text` into the app's memory at `out` when it fits in `cap`; its length.
    const give = (text, out, cap) => {
      const m = mem();
      if (cap >= text.length) {
        // Written in place when it fits (each UTF-16 unit takes at most 3 bytes: retry if short).
        const { read, written } = enc.encodeInto(text, m.subarray(out, out + cap));
        if (read === text.length) return written;
      }
      return enc.encode(text).length;
    };
    // A header name in the app's memory: ASCII is read a byte at a time (quicker than a TextDecoder).
    const named = (p, n) => {
      const m = mem();
      let s = '';
      for (let i = 0; i < n; i++) {
        const c = m[p + i];
        if (c > 127) return dec.decode(m.subarray(p, p + n));
        s += String.fromCharCode(c);
      }
      return s;
    };
    x.idle = () => (x.work ? new Promise((r) => x.idlers.push(r)) : settled);
    // Runs `f` once `promise` settles, counted as work until then.
    x.later = (promise, f) => {
      x.work++;
      promise.then(f).finally(() => --x.work || x.idlers.splice(0).forEach((r) => r()));
    };
    x.call = (f, a, b) => {
      try {
        f(a, b);
      } catch (e) {
        console.error(`wisp: the app failed (${e}); a new instance takes the next requests`);
        x.retired = true;
        const id = x.exports.wisp_current();
        const fail = (id) => {
          const done = x.of(id);
          if (typeof done === 'function') done(null);
          else if (done !== undefined) sink(done, failedHead, failed.body);
          x.done(id);
          x.streams.get(id)?.error(e);
          x.streams.delete(id);
          // A connection's task (`1 << 30` and its id): the socket goes.
          const io = id >= CONN && id < 0x80000000 ? x.conns.get(id - CONN) : null;
          if (io) x.conns.delete(id - CONN), io.destroy();
        };
        if (id !== 0xffffffff) {
          // One task trapped (a request's, or a `wisp::spawn`'s): the others go on.
          fail(id);
          x.call(x.exports.wisp_poll);
        } else {
          if (x.curId >= 0) fail(x.curId);
          [...x.pending.keys(), ...x.streams.keys()].forEach(fail);
        }
      }
    };
    const imports = {
      wisp: {
        random: (p, n) => void crypto.getRandomValues(mem().subarray(p, p + n)),
        now: () => Date.now() / 1000,
        log: (p, n) => console.error(dec.decode(copy(p, n))),
        // The head's text, its number among the app's (`2**32 - 1`: not kept),
        // and the body, all in the app's memory.
        reply: (id, hp, hn, hid, bp, bn) => {
          const done = x.of(id);
          x.done(id);
          const m = mem();
          if (typeof done === 'function' || done === undefined) {
            const all = new Uint8Array(hn + 1 + bn); // the head ends in a line, then a blank one
            all.set(m.subarray(hp, hp + hn));
            all.set(m.subarray(bp, bp + bn), hn + 1);
            all[hn] = 10;
            const r = decode(all);
            if (r.first.endsWith(' stream')) r.body = x.stream(id);
            return done?.(r);
          }
          const h = hid === 0xffffffff ? parsed(dec.decode(m.subarray(hp, hp + hn))) : (x.heads[hid] ??= parsed(dec.decode(m.subarray(hp, hp + hn))));
          sink(done, h, h.stream ? x.stream(id) : m.subarray(bp, bp + bn));
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
        // A connection's bytes, copied before this returns: 0 when it is behind.
        // An empty write ends it.
        conn_write: (id, p, n) => {
          const io = x.conns.get(id);
          if (!io) return 1;
          if (n) return io.write(mem().subarray(p, p + n)) ? 1 : 0;
          x.conns.delete(id);
          io.end();
          return 1;
        },
        // A `wisp_request_lazy` request's header, or all of them as lines
        // (but the framing ones, `host` added if it has none): the length,
        // written to `out` if it fits in `cap`; 2**32 - 1 for none.
        header: (id, np, nn, out, cap) => {
          let v = null;
          const c = x.of(id);
          const name = named(np, nn);
          // `:peer`, no header's name: the client's address, asked for once read.
          if (name === ':peer') v = typeof c?.peer === 'function' ? c.peer() || null : null;
          else
            try {
              v = c?.request?.headers.get(name) ?? null;
            } catch {} // not a header's name
          return v === null ? 0xffffffff : give(v, out, cap);
        },
        headers: (id, out, cap) => {
          const r = x.of(id);
          if (!r?.request) return 0xffffffff;
          let t = '';
          let host = false;
          for (const [k, v] of r.request.headers) {
            if (k === 'content-length' || k === 'transfer-encoding') continue;
            host ||= k === 'host';
            t += `${k}: ${v}\n`;
          }
          return give(host ? t : t + `host: ${r.host}\n`, out, cap);
        },
        // A message for the WebSocket of request `id`, copied: sent when the host has made it.
        ws: (id, op, p, n) => {
          let s = x.socks.get(id);
          if (!s) x.socks.set(id, (s = { ws: null, q: [] }));
          const bytes = copy(p, n);
          if (s.ws && s.ws.readyState !== 0) put(s.ws, op, bytes);
          else s.q.push([op, bytes]);
        },
        timer: (id, ms) => x.later(new Promise((r) => setTimeout(r, ms)), () => x.call(() => x.exports.wisp_timer(id))),
      },
    };
    x.exports = (await WebAssembly.instantiate(module, imports)).exports;
    const vars = Object.entries(env).filter(([k, v]) => typeof v === 'string' && k !== 'WISP_WS');
    // Windows names are any case (`Path`): the app reads them upper case, as natively.
    if (globalThis.process?.platform === 'win32' || globalThis.Deno?.build?.os === 'windows') {
      const have = new Set(vars.map(([k]) => k));
      for (const [k, v] of [...vars]) if (!have.has(k.toUpperCase())) have.add(k.toUpperCase()), vars.push([k.toUpperCase(), v]);
    }
    vars.push(['WISP_WS', accept ? '1' : '0']);
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
    let h = `${method} ${target} ${peer}\n`;
    for (const [k, v] of headers) h += `${k}: ${v}\n`;
    x.call(x.exports.wisp_request, id, x.request(h + '\n', body));
    const r = await answer;
    const idle = x.idle();
    return r ? { status: parseInt(r.first) || 500, headers: r.headers, body: r.body, idle, id, x } : { ...failed, idle };
  }

  // The app answered `request` 101: the host's `accept` makes the socket, and
  // the app's messages and the client's go between them. Returns what the
  // host's handler returns.
  function takeover(x, id, request) {
    let ws, response;
    try {
      ({ ws, response } = accept(request));
    } catch {
      x.call(() => x.exports.wisp_cancel(id)); // not an upgrade after all
      return new Response('Bad Request', { status: 400 });
    }
    let s = x.socks.get(id);
    if (!s) x.socks.set(id, (s = { ws: null, q: [] }));
    s.ws = ws;
    try {
      ws.binaryType = 'arraybuffer';
    } catch {}
    const flush = () => {
      if (ws.readyState !== 0) for (const [op, bytes] of s.q.splice(0)) put(ws, op, bytes);
    };
    ws.addEventListener('open', flush);
    ws.addEventListener('message', (e) => {
      const text = typeof e.data === 'string';
      const bytes = text ? enc.encode(e.data) : new Uint8Array(e.data);
      x.call(() => x.exports.wisp_ws_message(id, text ? 1 : 2, x.put(bytes)));
    });
    const end = () => x.socks.delete(id) && x.call(() => x.exports.wisp_ws_close(id));
    // The client closed: tell the app, then answer the close where the host
    // leaves that to us (workerd holds the request open until `close`, and
    // cancels it as hung otherwise). The app's own close, sent by `end`, wins.
    ws.addEventListener('close', (e) => {
      end();
      try {
        if (ws.readyState !== 3) ws.close(e.code >= 1000 && e.code < 5000 && ![1004, 1005, 1006, 1015].includes(e.code) ? e.code : 1000, e.reason);
      } catch {} // closed already
    });
    ws.addEventListener('error', end);
    flush();
    return response;
  }

  // A bodyless request, headers as Node's `rawHeaders` (name, value, name, ...),
  // answered through `sink` with `ctx`. Synchronous when the app is, and nothing
  // is made per request but the request's text. False when no live instance is
  // ready: use `handle`.
  function direct(method, target, peer, raw, ctx) {
    const x = ready;
    if (!x || x.retired) return false;
    const id = (next = (next + 1) & 0x7fffffff);
    x.pending.set(id, ctx);
    // A client sends the same request again and again: its bytes are kept, by
    // target, and used when the method, peer and every header are the same.
    let c = seen.get(target);
    if (!(c && c.method === method && c.peer === peer && same(c.raw, raw))) {
      let h = `${method} ${target} ${peer}\n`;
      for (let i = 0; i < raw.length; i += 2) h += `${raw[i]}: ${raw[i + 1]}\n`;
      c = { method, peer, raw: raw.slice(), bytes: enc.encode(h + '\n') };
      if (seen.size > 64) seen.clear();
      seen.set(target, c);
    }
    x.call(x.exports.wisp_request, id, x.write(c.bytes));
    return true;
  }

  // A socket the host accepted, whose bytes Wisp parses and answers itself
  // (the instance's `Raw`): `io` is { write(bytes) -> false when behind,
  // end(), destroy() }, `bytes` a view of the app's memory, good only until
  // `write` returns. Returns { data(bytes), drain(), close() } for the host's
  // read, drain and close events, or null with no live instance: use `fetch`.
  let conns = 0;
  function conn(io, peer = '') {
    const x = ready;
    if (!x || x.retired) return void instance().catch(() => {}); // the next one finds it
    let id;
    do id = conns = (conns + 1) & (CONN - 1);
    while (x.conns.has(id));
    x.conns.set(id, io);
    x.call(x.exports.wisp_conn_open, id, x.write(enc.encode(peer)));
    return {
      data: (bytes) => x.call(x.exports.wisp_conn_data, id, x.write(bytes)),
      drain: () => x.call(x.exports.wisp_conn_pull, id),
      close: () => x.conns.delete(id) && x.call(x.exports.wisp_conn_close, id, 0),
    };
  }

  // Whether the server at `url` answers `GET /` over a real HTTP client as
  // the app does itself, twice (the second on the kept-alive connection).
  // `get(url)` is that client, resolving to the status: `fetch` unless the
  // host has a cheaper one (Node's loads undici, tens of ms, at first use).
  async function check(url, get = fetched) {
    try {
      const want = await handle({ method: 'GET', target: '/', headers: [['host', new URL(url).host]], body: none });
      if (want.body instanceof ReadableStream) want.body.cancel();
      for (let i = 0; i < 2; i++) if ((await get(url)) !== want.status) return false;
      return true;
    } catch {
      return false;
    }
  }

  const up = accept && ((c) => takeover(ready, c.id, c.request));

  // A web `Request` to a web `Response`: Workers, Deno, Netlify. `ctx` is
  // the host's context, whose `waitUntil` keeps background work alive.
  // A request without a body goes straight to the app and, when the app
  // answers at once, comes back as a Response with no Promise made. Its
  // headers are not read here (`host` is the URL's, and Bun and Deno make
  // `request.headers` only once read): the app asks for those it reads.
  // `peer` is the client's address, or a function giving it, called only
  // when the app is asked (Bun's `requestIP` costs; a kept answer skips it).
  function serve(request, peer = '', ctx) {
    const x = ready;
    if (!x || x.retired || request.body) return slow(request, typeof peer === 'function' ? peer() : peer, ctx);
    const method = request.method;
    const url = request.url;
    const s = url.indexOf('//') + 2;
    const at = url.indexOf('/', s);
    const path = at < 0 ? '/' : url.slice(at);
    const get = method === 'GET';
    if ((get || method === 'HEAD') && fast.size) {
      const f = fast.get(path);
      const headers = f && request.headers;
      if (f && !headers.has('x-wisp-error')) {
        const tags = f.etag && headers.get('if-none-match');
        if (!tags) return new Response(get ? f.body : null, f.init);
        if (f.nm) return names(tags, f.etag) ? new Response(null, f.nm) : new Response(get ? f.body : null, f.init);
      }
    }
    const host = url.slice(s, at < 0 ? url.length : at);
    // A function `peer` is called only if the app reads the address (`:peer`).
    const c = { res: null, resolve: null, empty: method === 'HEAD', fast: get && !path.includes('?') ? fast : null, path, probe, request, host, up, id: 0, peer };
    enter(x, method, path, typeof peer === 'function' ? '' : peer, c);
    if (x.work) ctx?.waitUntil?.(x.idle());
    return c.res ?? new Promise((resolve) => (c.resolve = resolve));
  }

  // The request `method path` to `c.host`, its other headers read from
  // `c.request.headers` (made by the host only once read) as the app asks
  // for them, answered through `c`.
  function enter(x, method, path, peer, c) {
    // Kept by path, the rest compared: no text is made and hashed per
    // request (a fifth of the shim's own JS on Bun).
    let e = sent.get(path);
    if (!(e && e.method === method && e.peer === peer && e.host === c.host)) {
      if (sent.size > 64) sent.clear();
      const bytes = enc.encode(`${method} ${path} ${peer}\nhost: ${c.host}\n\n`);
      sent.set(path, (e = { method, peer, host: c.host, bytes }));
    }
    const id = (next = (next + 1) & 0x7fffffff);
    c.id = id;
    x.cur = c;
    x.curId = id;
    x.call(x.exports.wisp_request_lazy, id, x.write(e.bytes));
    // Not answered in the call (no Map work for one that was): it waits.
    if (x.curId === id) x.pending.set(id, c);
    x.cur = null;
    x.curId = -1;
  }

  // Asks the app for the 304 of a kept path (the first, only, time).
  function probe(entry) {
    const x = ready;
    if (!x || x.retired || !entry.etag) return;
    const path = [...fast].find(([, e]) => e === entry)?.[0];
    if (path === undefined) return;
    const headers = new Headers({ host: 'wisp.invalid', 'if-none-match': entry.etag });
    enter(x, 'GET', path, '', { quiet: true, fast, entry, path, request: { headers }, host: 'wisp.invalid' });
  }

  // A request's body, read to one byte past its route's limit at most: the
  // app answers 413 for that, as its own server does, and the rest of a
  // large body is never held.
  async function bounded(request, path) {
    let x = ready;
    if (!x || x.retired) x = await instance().catch(() => null);
    // A route's limit is fixed for the instance: asked once a path.
    let cap = Infinity;
    if (x) {
      x.limits ??= new Map();
      cap = x.limits.get(path);
      if (cap === undefined) {
        if (x.limits.size > 256) x.limits.clear();
        x.limits.set(path, (cap = (x.exports.wisp_body_limit(x.write(enc.encode(path))) >>> 0) + 1));
      }
    }
    const length = request.headers.get('content-length');
    if (length !== null && Number(length) < cap) return new Uint8Array(await request.arrayBuffer());
    const parts = [];
    let n = 0;
    const reader = request.body.getReader();
    while (n < cap) {
      const { done, value } = await reader.read();
      if (done) break;
      parts.push(value.subarray(0, cap - n));
      n += parts.at(-1).length;
    }
    if (n >= cap) reader.cancel().catch(() => {});
    const out = new Uint8Array(n);
    n = 0;
    for (const p of parts) out.set(p, (n += p.length) - p.length);
    return out;
  }

  async function slow(request, peer, ctx) {
    const url = new URL(request.url);
    const headers = [...request.headers];
    if (!request.headers.has('host')) headers.push(['host', url.host]);
    const body = request.body ? await bounded(request, url.pathname) : none;
    const r = await handle({ method: request.method, target: url.pathname + url.search, peer, headers, body });
    if (r.status === 101) return accept ? takeover(r.x, r.id, request) : new Response('WebSockets are not available on this host', { status: 501 });
    if (r.idle !== settled) ctx?.waitUntil?.(r.idle); // only when work is under way
    const empty = bare(r.status, request.method === 'HEAD', r.body);
    return new Response(empty ? null : r.body, { status: r.status, headers: r.headers });
  }

  return { handle, direct, fetch: serve, conn, check, up: instance };
}
