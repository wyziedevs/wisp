// node score.mjs <host>: number of unranked cells of results/<host>.json (+1000 when the host file is invalid); 0 = fully valid.
import { readFileSync } from 'node:fs';
import { hostWhy, cellWhy } from './lib.mjs';
const r = JSON.parse(readFileSync(new URL(`./results/${process.argv[2]}.json`, import.meta.url), 'utf8'));
console.log((hostWhy(r) ? 1000 : 0) + Object.values(r.cells).filter((c) => cellWhy(c)).length + Object.keys(r.failed).length);
