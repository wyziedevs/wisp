// A Node server for a Wisp app: `node server.mjs` listens on $PORT (3000)
// and $HOST (0.0.0.0). `handler` is the same app as a Node (req, res)
// function, for Vercel, Firebase and anything built on node:http.
import { readFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { pathToFileURL } from 'node:url';
import { wisp } from './bridge.mjs';

const app = wisp(await WebAssembly.compile(readFileSync(new URL('./app.wasm', import.meta.url))), process.env);

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

export default async function handler(req, res) {
  let body = req.rawBody; // Firebase and Google Cloud have read it already
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
  const r = await app.handle({ method: req.method, target: req.url, peer, headers, body: new Uint8Array(body) });
  res.writeHead(r.status, r.headers.flat());
  res.end(req.method === 'HEAD' ? undefined : r.body);
}

export { app };

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const port = Number(process.env.PORT) || 3000;
  const host = process.env.HOST || '0.0.0.0';
  createServer(handler).listen(port, host, () => console.log(`wisp: listening on http://${host}:${port}`));
}
