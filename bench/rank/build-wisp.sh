#!/bin/sh
# Builds the Wisp bench app for every host into out/wisp-{node,bun,deno,cf}.
# Needs a `wisp` binary (cargo build --release -p wisp-cli) and its Rust
# toolchain with the wasm target. WISP=<path to wisp> sh build-wisp.sh
set -e
here=$(cd "$(dirname "$0")" && pwd)
wisp=$(cd "$(dirname "${WISP:-wisp}")" && pwd)/$(basename "${WISP:-wisp}")
work="$here/.wisp-app"
rm -rf "$work"
mkdir -p "$work" "$here/out"
cd "$work"
"$wisp" new app >/dev/null
cd app
rm -rf src/routes static
cp -r "$here/../edge/apps/wisp/src/routes" src/routes
for t in node bun deno cloudflare; do
  d="$here/out/wisp-$t"
  [ "$t" = cloudflare ] && d="$here/out/wisp-cf"
  rm -rf "$d"
  "$wisp" build --target "$t" --out "$d"
done
echo '{"entry":"worker.js","date":"2025-09-01"}' > "$here/out/wisp-cf/meta.json"
echo "wisp builds in $here/out"
