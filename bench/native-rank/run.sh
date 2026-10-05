#!/bin/bash
# One-command reproduce, ON a Linux host with >= 4 cores (server on 0-1, oha on 2-3):
#   ./run.sh all           setup tools, build, verify, pass 1, pass 2 (reverse order), report
#   ./run.sh build|verify|pass 1|pass 2|pass 3|report   (pass 3 is the tie-break; STEAL_MAX/STEAL_TRIES tune the steal gate)
# Every measurement runs under flock /tmp/wisp-bench.lock (the host is shared). Pass 2 walks the
# frameworks in the opposite order, and should be started at a different time than pass 1.
set -u
cd "$(dirname "$0")" || exit 1
export PATH=/root/.cargo/bin:/root/nr-tools/go/bin:/root/dotnet:/usr/local/bin:/opt/bun/bin:$PATH
LOCK=/tmp/wisp-bench.lock
ALL="wisp aspnet axum actix gin fastify express hono-bun spring fastapi next sveltekit"
ulimit -n 1048576 2>/dev/null || ulimit -n 65535

pass() {
  local n=$1 list=$ALL
  [ $((n % 2)) = 0 ] && list=$(echo $ALL | tr ' ' '\n' | tac | tr '\n' ' ')
  # pass 3 (the tie-break when 1 and 2 disagree): neither order, Rust servers split up
  [ "$n" = 3 ] && list="axum fastify wisp spring actix sveltekit aspnet fastapi gin next hono-bun express"
  for f in $list; do
    flock $LOCK node run.mjs bench --pass "$n" --fw "$f"
  done
}

case ${1:-all} in
  setup) bash setup.sh ;;
  build) node run.mjs build ;;
  verify) flock $LOCK node run.mjs verify ;;
  pass) pass "${2:-1}" ;;
  report) node report.mjs > results/report.md && echo "wrote results/report.md" ;;
  all) bash setup.sh; node run.mjs build; flock $LOCK node run.mjs verify && pass 1 && pass 2; node report.mjs > results/report.md ;;
esac
