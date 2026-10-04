#!/bin/sh
# Runs the whole ranking bench, one host after the other, then renders the tables.
# The box should be idle. WORKERD_BIN (default: the one under apps/micro), BUN_BIN, DENO_BIN.
#   ./run.sh [rank.mjs options, e.g. --runs 5 --secs 10]
here=$(cd "$(dirname "$0")" && pwd)
cd "$here"
export WORKERD_BIN="${WORKERD_BIN:-$here/apps/micro/node_modules/.bin/workerd}"
for h in node bun deno workerd; do
  node rank.mjs --host "$h" "$@" || echo "host $h failed" >&2
done
node report.mjs > results/report.md
echo "tables in $here/results/report.md"
