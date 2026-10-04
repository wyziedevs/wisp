// Cloudflare Workers entry: static files in ./public are served before
// this runs; everything else reaches the app.
import module from './app.wasm';
import { wisp } from './bridge.mjs';

let app;

// A WebSocket the app answered. The isolate holds it, so what the app keeps
// across connections lives in this isolate alone (use a Durable Object for
// state shared by every connection).
function accept() {
  const { 0: client, 1: server } = new WebSocketPair();
  server.accept();
  return { ws: server, response: new Response(null, { status: 101, webSocket: client }) };
}

export default {
  fetch(request, env, ctx) {
    app ??= wisp(module, env, undefined, accept);
    return app.fetch(request, request.headers.get('cf-connecting-ip') ?? '', ctx);
  },
  // A cron trigger of wrangler.toml: asks the app for `/_wisp/cron/<schedule>`
  // (fields joined by `_`, `/` as `~`), which runs its `wisp::cron` tasks and
  // the queues' due jobs.
  async scheduled(event, env, ctx) {
    app ??= wisp(module, env, undefined, accept);
    const slug = event.cron.trim().split(/\s+/).join('_').replaceAll('/', '~');
    const headers = { authorization: `Bearer ${env.CRON_SECRET}` };
    const res = await app.fetch(new Request(`https://wisp.invalid/_wisp/cron/${slug}`, { headers }), '', ctx);
    if (!res.ok) throw new Error(`cron ${event.cron}: ${res.status}`);
  },
};
