// node provenance.mjs <wisp commit> : writes out/provenance.json after a build: the Wisp commit measured,
// the build date and every contender app's resolved npm versions (`npm ls --depth=0 --json`), which rank.mjs
// copies into each results file.
import { writeFileSync, existsSync, mkdirSync } from 'node:fs';
import { execSync } from 'node:child_process';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { npmVersions } from './lib.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const contenders = {};
for (const a of ['micro', 'sveltekit', 'next', 'astro', 'rr']) {
  const d = join(here, 'apps', a);
  if (!existsSync(join(d, 'node_modules'))) continue;
  try { contenders[a] = npmVersions(JSON.parse(execSync('npm ls --depth=0 --json', { cwd: d, encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }))); } catch {}
}
mkdirSync(join(here, 'out'), { recursive: true });
writeFileSync(join(here, 'out', 'provenance.json'), JSON.stringify({ wisp_commit: process.argv[2] || null, build_date: new Date().toISOString().slice(0, 10), contenders }, null, 1));
