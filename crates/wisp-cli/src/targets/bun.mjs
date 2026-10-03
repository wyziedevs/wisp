// A Bun server for a Wisp app: `bun server.mjs` listens on $PORT (3000)
// and $HOST (0.0.0.0) with Bun.serve, which is faster there than node:http.
import { wisp } from './bridge.mjs';

const app = wisp(await WebAssembly.compile(await Bun.file(`${import.meta.dir}/app.wasm`).arrayBuffer()), process.env);

Bun.serve({
  port: Number(process.env.PORT) || 3000,
  hostname: process.env.HOST || '0.0.0.0',
  fetch: (request, server) => app.fetch(request, server.requestIP(request)?.address ?? ''),
});
