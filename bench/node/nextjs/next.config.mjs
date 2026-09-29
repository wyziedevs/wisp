import { dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

// standalone: a self-contained server.js that bench/node/cluster.mjs can run.
// The root is bench/node so the page can import the shared fortunes.mjs.
const root = dirname(dirname(fileURLToPath(import.meta.url)));
export default { output: 'standalone', outputFileTracingRoot: root, turbopack: { root } };
