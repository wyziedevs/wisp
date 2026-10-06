#!/bin/sh
# Installs (npm ci from the committed lockfiles) and builds every framework app of the ranking bench into out/.
# Run on the Linux box (node >= 20, npm, bun and deno on PATH or in BUN_BIN /
# DENO_BIN). Wisp's builds (build-wisp.sh) are expected in out/wisp-* already.
#   WISP_COMMIT=<sha of the Wisp build in out/> sh build.sh   everything (the sha goes in out/provenance.json)
#   sh build.sh micro rr   only those (micro sveltekit next astro rr fresh)
set -e
here=$(cd "$(dirname "$0")" && pwd)
cd "$here"
apps="${*:-micro sveltekit next astro rr fresh}"
mkdir -p out
has() { case " $apps " in *" $1 "*) return 0;; esac; return 1; }
common() { cp apps/common/data.mjs "$1"; }
# wrangler bundles a worker exactly as it ships; the bench serves that bundle on bare workerd.
bundle() { # <app dir> <wrangler config> <out dir> <compat flags, json list>
  rm -rf "$3"
  (cd "$1" && npx wrangler deploy --dry-run --outdir "$3" -c "$2" >/dev/null)
  echo "{\"entry\":\"$4\",\"date\":\"2025-09-01\",\"flags\":$5}" > "$3/meta.json"
}

if has micro; then
  cp ../edge/apps/hono/app.mjs apps/micro/hono-app.mjs
  common apps/micro
  (cd apps/micro && npm ci --no-audit --no-fund --silent)
  rm -rf out/micro && mkdir out/micro
  cp apps/micro/*.mjs apps/micro/*.ts apps/micro/package.json out/micro/
  ln -sfn "$here/apps/micro/node_modules" out/micro/node_modules
  # Oak is JSR-only and Deno does not mix that with a package.json: its own dir
  rm -rf out/oak && mkdir out/oak && cp apps/micro/oak.ts apps/micro/data.mjs out/oak/
  (cd out/oak && "${DENO_BIN:-deno}" cache oak.ts)
  bundle "$here/apps/micro" wrangler.hono.toml "$here/out/hono-cf" hono-app.js '[]'
  bundle "$here/apps/micro" wrangler.itty.toml "$here/out/itty-cf" itty.js '[]'
fi

if has sveltekit; then
  mkdir -p apps/sveltekit/src/lib && common apps/sveltekit/src/lib
  (cd apps/sveltekit && npm ci --no-audit --no-fund --silent)
  (cd apps/sveltekit && rm -rf build .svelte-kit && npx vite build >/dev/null)
  rm -rf out/sveltekit-node && cp -r apps/sveltekit/build out/sveltekit-node
  cp apps/sveltekit/package.json out/sveltekit-node/package.json
  ln -sfn "$here/apps/sveltekit/node_modules" out/sveltekit-node/node_modules
  (cd apps/sveltekit && rm -rf .svelte-kit && ADAPTER=cloudflare npx vite build >/dev/null)
  bundle "$here/apps/sveltekit" wrangler.jsonc "$here/out/sveltekit-cf" _worker.js '["nodejs_compat"]'
fi

if has next; then
  mkdir -p apps/next/lib && common apps/next/lib
  (cd apps/next && npm ci --no-audit --no-fund --silent && rm -rf .next .open-next && npx next build >/dev/null)
  rm -rf out/next-node && cp -r apps/next/.next/standalone out/next-node
  mkdir -p out/next-node/.next && cp -r apps/next/.next/static out/next-node/.next/static
  # opennext reuses the build; skipped with a note when it fails
  if (cd apps/next && npx opennextjs-cloudflare build --skipNextBuild >/dev/null 2>&1); then
    bundle "$here/apps/next" wrangler.jsonc "$here/out/next-cf" worker.js '["nodejs_compat","global_fetch_strictly_public"]'
  else
    echo "next on cloudflare did not build; skipped" >&2
  fi
fi

if has astro; then
  common apps/astro/src
  (cd apps/astro && npm ci --no-audit --no-fund --silent && rm -rf dist && npx astro build >/dev/null)
  # the adapter writes a ready worker (no_bundle: entry.mjs plus chunks)
  rm -rf out/astro-cf && cp -r apps/astro/dist/server out/astro-cf
  echo '{"entry":"entry.mjs","date":"2025-09-01","flags":[]}' > out/astro-cf/meta.json
fi

if has rr; then
  common apps/rr/app
  (cd apps/rr && npm ci --no-audit --no-fund --silent && rm -rf build && npx react-router build >/dev/null)
  bundle "$here/apps/rr" build/server/wrangler.json "$here/out/rr-cf" index.js '["nodejs_compat"]'
fi
if has fresh; then
  common apps/fresh
  (cd apps/fresh && "${DENO_BIN:-deno}" install >/dev/null 2>&1 && rm -rf _fresh && "${DENO_BIN:-deno}" task build >/dev/null 2>&1)
  rm -rf out/fresh && ln -sfn "$here/apps/fresh" out/fresh
fi
WISP_COMMIT="${WISP_COMMIT:-}" node provenance.mjs "$WISP_COMMIT"
echo "built: $apps"
