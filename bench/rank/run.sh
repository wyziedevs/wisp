#!/bin/sh
# Runs the whole ranking bench, one host after the other, then renders the tables.
# The box should be idle. WORKERD_BIN (default: the one under apps/micro), BUN_BIN, DENO_BIN.
# vmstat -t samples CPU steal (st column, per second, timestamped) during each host's run (mark.mjs stores it and marks
# the run valid when mean steal is 10% or less; report.mjs ranks only valid runs).
#   [HOSTS="node bun"] ./run.sh [rank.mjs options, e.g. --runs 5 --secs 10]
here=$(cd "$(dirname "$0")" && pwd)
cd "$here"
export WORKERD_BIN="${WORKERD_BIN:-$here/apps/micro/node_modules/.bin/workerd}"
mkdir -p results
for h in ${HOSTS:-node bun deno workerd}; do
  vmstat -t 1 > "results/$h.vmstat" &
  vm=$!
  node rank.mjs --host "$h" "$@" || echo "host $h failed" >&2
  kill $vm
  node mark.mjs "$h" "results/$h.vmstat"
done
node report.mjs > results/report.md
echo "tables in $here/results/report.md"
