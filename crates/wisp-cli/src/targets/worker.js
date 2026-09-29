// Cloudflare Workers entry: static files in ./public are served before
// this runs; everything else reaches the app.
import module from './app.wasm';
import { wisp } from './bridge.mjs';

let app;

export default {
  fetch(request, env, ctx) {
    app ??= wisp(module, env);
    return app.fetch(request, request.headers.get('cf-connecting-ip') ?? '', ctx);
  },
};
