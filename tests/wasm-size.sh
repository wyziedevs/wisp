#!/bin/sh
# Fails when the tests app's wasm, built as `wisp build --target cloudflare`
# builds it (release, stripped, request-only), is over BUDGET bytes. A host
# loads the file on every cold start, and it grew 8% once without
# anyone choosing it (WebSockets, jobs, i18n, raw connections in every build).
# BUDGET is the size after the last cut plus 2%: raise it on purpose, in the
# commit that adds the code, never to make a red build green.
# Cold start is not gated: workerd's is bimodal on one machine (25 or 32 ms
# for the same bytes) and compiles lazily, so a time bound would flake.
set -eu
BUDGET=1810000
export WISP_REQUEST_ONLY=1 CARGO_PROFILE_RELEASE_STRIP=symbols
cargo build -q --release -p wisp-test-app --target wasm32-unknown-unknown
size=$(wc -c < target/wasm32-unknown-unknown/release/wisp-test-app.wasm)
echo "wasm: $size bytes (budget $BUDGET)"
[ "$size" -le "$BUDGET" ] || { echo "the wasm is over budget" >&2; exit 1; }
