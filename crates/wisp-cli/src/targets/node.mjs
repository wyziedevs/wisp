// A Node server for a Wisp app: `node server.mjs` listens on $PORT (3000)
// and $HOST (0.0.0.0). `handler` is the same app as a Node (req, res)
// function, for Vercel, Firebase and anything built on node:http.
//
// The server reads and writes raw sockets and the app's own HTTP parser
// answers: no node:http objects per request. It is used once a request over
// loopback has been answered as the app answers it; WISP_NODE_HTTP=1, or a
// failed check, serves with node:http instead.
import { readFileSync } from 'node:fs';
import { Agent, createServer, get } from 'node:http';
import { createServer as tcp } from 'node:net';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { pathToFileURL } from 'node:url';
import { wisp } from './bridge.mjs';

const none = new Uint8Array();
const app = wisp(await WebAssembly.compile(readFileSync(new URL('./app.wasm', import.meta.url))), process.env, send);

// The most of a request body read before the app sees it: WISP_BODY_LIMIT
// (`1048576`, `512KB`, `10MB`, `1GB`), 1 MB by default, as the app's own
// server has it. A route's larger `BODY_LIMIT` needs WISP_BODY_LIMIT raised.
const limit = (() => {
  const m = /^\s*(\d+)\s*(GB|MB|KB|B)?\s*$/i.exec(process.env.WISP_BODY_LIMIT ?? '');
  return m ? Number(m[1]) * { GB: 2 ** 30, MB: 2 ** 20, KB: 2 ** 10, B: 1 }[(m[2] ?? 'B').toUpperCase()] : 2 ** 20;
})();

function tooLarge(req, res) {
  res.writeHead(413, { 'content-type': 'text/plain; charset=utf-8', connection: 'close' });
  res.end('Payload Too Large', () => req.destroy());
}

// The answer to `app.direct`, no web Response in between: the head's
// header array goes to Node as it is, the body is copied out of the app's
// memory (a socket may hold it past this call, and the memory moves on).
function send(res, h, body) {
  const m = res.req.method;
  let out = h.headers;
  if (body instanceof Uint8Array && h.status >= 200 && h.status !== 204 && h.status !== 304) {
    // A whole body gets a length, so Node writes it in one piece, not chunked.
    // `h.sized` is the head's array with a length slot, filled in here.
    h.sized ??= out.some((k, i) => !(i & 1) && k.toLowerCase() === 'content-length') ? out : [...out, 'content-length', 0];
    out = h.sized;
    if (out !== h.headers) out[out.length - 1] = body.length;
  }
  res.writeHead(h.status, out);
  if (body instanceof Uint8Array) {
    if (m === 'HEAD' || h.status < 200 || h.status === 204 || h.status === 304) return res.end();
    return body.length < 4096 ? res.end(latin1.call(body, 0, body.length), 'latin1') : res.end(Buffer.from(body));
  }
  if (m === 'HEAD') return body.cancel().finally(() => res.end());
  res.flushHeaders();
  pipeline(Readable.fromWeb(body), res).catch(() => {});
}

// A small body as a latin1 string, byte for byte: Node writes a string with
// the head in one piece, where a Buffer goes out as a second chunk.
const latin1 = Buffer.prototype.latin1Slice;

// A request with no body to read: neither content-length nor transfer-encoding.
function bodiless(raw) {
  for (let i = 0; i < raw.length; i += 2) {
    const k = raw[i];
    if ((k.length === 14 || k.length === 17) && /^(content-length|transfer-encoding)$/i.test(k)) return false;
  }
  return true;
}

export default function handler(req, res) {
  if (!req.rawBody && bodiless(req.rawHeaders) && app.direct(req.method, req.url, req.socket?.remoteAddress ?? '', req.rawHeaders, res)) return;
  return slow(req, res);
}

async function slow(req, res) {
  let body = req.rawBody; // Firebase and Google Cloud have read it already
  if (!body && !req.headers['content-length'] && !req.headers['transfer-encoding']) body = none; // most GETs
  if (!body) {
    if (Number(req.headers['content-length']) > limit) return tooLarge(req, res);
    const chunks = [];
    let size = 0;
    for await (const c of req) {
      size += c.length;
      if (size > limit) return tooLarge(req, res);
      chunks.push(c);
    }
    body = Buffer.concat(chunks);
  }
  const headers = [];
  for (let i = 0; i < req.rawHeaders.length; i += 2) headers.push([req.rawHeaders[i], req.rawHeaders[i + 1]]);
  const peer = req.socket?.remoteAddress ?? '';
  const r = await app.handle({ method: req.method, target: req.url, peer, headers, body });
  const out = r.headers.flat();
  const whole = !(r.body instanceof ReadableStream);
  // A whole body gets a length, so Node writes it in one piece, not chunked.
  if (whole && r.status >= 200 && r.status !== 204 && r.status !== 304 && !r.headers.some(([k]) => k.toLowerCase() === 'content-length')) out.push('content-length', r.body.length);
  res.writeHead(r.status, out);
  if (whole) return res.end(req.method === 'HEAD' ? undefined : r.body);
  if (req.method === 'HEAD') return r.body.cancel().finally(() => res.end());
  // Streamed: each chunk goes out as it comes. A client that leaves
  // cancels the stream, which the app sees as its sender failing.
  res.flushHeaders();
  return pipeline(Readable.fromWeb(r.body), res).catch(() => {});
}

export { app };

// A socket's bytes to the app and its answers back. A connection the app
// cannot take (no live instance) goes to node:http, as it is.
const http = createServer(handler);
const IDLE = 60_000;
let raws = 0;

function accept(socket, first) {
  const c = app.conn(
    {
      // A view of the app's memory: copied here, small ones as a string, which
      // Node writes without a Buffer.
      write(v) {
        const ok = socket.write(v.length < 16384 ? latin1.call(v, 0, v.length) : Buffer.from(v), v.length < 16384 ? 'latin1' : undefined);
        if (!ok) socket.pause();
        return ok;
      },
      end: () => socket.end(),
      destroy: () => socket.destroy(),
    },
    socket.remoteAddress ?? '',
  );
  if (!c) return first ? socket.destroy() : http.emit('connection', socket);
  raws++;
  socket.setTimeout(IDLE, () => socket.destroy());
  socket.on('data', c.data);
  socket.on('drain', () => (socket.resume(), c.drain()));
  socket.on('close', c.close);
  socket.on('error', () => {}); // 'close' follows
  if (first) (socket.resume(), c.data(first));
}

// An upgrade node:http saw (raw sockets are off, or the app was not live
// yet): the socket is the app's, and reads the handshake again from the
// request's text.
http.on('upgrade', async (req, socket, head) => {
  try {
    await app.up();
  } catch {
    return socket.destroy();
  }
  let text = `${req.method} ${req.url} HTTP/${req.httpVersion}\r\n`;
  for (let i = 0; i < req.rawHeaders.length; i += 2) text += `${req.rawHeaders[i]}: ${req.rawHeaders[i + 1]}\r\n`;
  accept(socket, Buffer.concat([Buffer.from(text + '\r\n', 'latin1'), head]));
});

// Whether raw sockets answer: a probe server on loopback, asked as a client
// would. The client is node:http's, already loaded: `fetch` would load undici.
async function verified() {
  const sockets = new Set();
  const probe = tcp({ noDelay: true }, (s) => (sockets.add(s), s.on('close', () => sockets.delete(s)), accept(s)));
  const agent = new Agent({ keepAlive: true, maxSockets: 1 });
  const status = (url) =>
    new Promise((ok, no) => {
      const req = get(url, { agent, timeout: 5000 }, (res) => res.resume().on('end', () => ok(res.statusCode)).on('error', no));
      req.on('timeout', () => req.destroy(new Error('timed out'))).on('error', no);
    });
  try {
    await new Promise((ok, no) => probe.once('error', no).listen(0, '127.0.0.1', ok));
    raws = 0;
    return (await app.check(`http://127.0.0.1:${probe.address().port}/`, status)) && raws > 0;
  } catch {
    return false;
  } finally {
    agent.destroy();
    sockets.forEach((s) => s.destroy());
    probe.close();
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const port = Number(process.env.PORT) || 3000;
  const host = process.env.HOST || '0.0.0.0';
  const forced = process.env.WISP_NODE_HTTP === '1';
  const raw = !forced && (await verified());
  console.error(`wisp: node serves with ${raw ? 'raw sockets' : 'node:http'} (${raw ? 'checked' : forced ? 'WISP_NODE_HTTP=1' : 'the check failed'})`);
  const server = raw ? tcp({ noDelay: true }, accept) : http;
  server.listen(port, host, () => console.log(`wisp: listening on http://${host}:${port}`));
}
