// Vercel Edge entry. The wasm is imported as a module, which the edge
// runtime compiles at deploy time. Files in `.vercel/output/static` are
// served first (`handle: filesystem`); everything else reaches the app.
import module from './app.wasm?module';
import { wisp } from './bridge.mjs';

const app = wisp(module, process.env);

export default (request, context) => app.fetch(request, request.headers.get('x-real-ip') ?? '', context);
