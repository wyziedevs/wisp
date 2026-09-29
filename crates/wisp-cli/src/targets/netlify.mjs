// Netlify Functions entry. Files in `public` are served first
// (`preferStatic`); everything else reaches the app. The wasm is inlined in
// `app.wasm.js`, since Netlify bundles functions.
import wasm from './app.wasm.js';
import { wisp } from './bridge.mjs';

const app = wisp(await WebAssembly.compile(Buffer.from(wasm, 'base64')), process.env);

export default (request, context) => app.fetch(request, context.ip ?? '', context);

export const config = { path: '/*', preferStatic: true };
