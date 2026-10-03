// Netlify Edge entry (Deno). The wasm is imported as a module. Edge
// functions run before static files, so the config skips the app's own.
import module from './app.wasm?module';
import { wisp } from './bridge.mjs';

const app = wisp(module, Deno.env.toObject());

export default (request, context) => app.fetch(request, context.ip ?? '', context);

export const config = { path: '/*', excludedPath: [/*SKIP*/] };
