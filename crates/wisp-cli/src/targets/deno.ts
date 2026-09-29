// Deno Deploy entry: `deno run --allow-read --allow-env --allow-net main.ts`
// locally, `deployctl deploy --entrypoint main.ts` to deploy.
import { wisp } from './bridge.mjs';

const wasm = await Deno.readFile(new URL('./app.wasm', import.meta.url));
const app = wisp(await WebAssembly.compile(wasm), Deno.env.toObject());

Deno.serve((request, info) => app.fetch(request, (info.remoteAddr as Deno.NetAddr).hostname));
