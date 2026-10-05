#!/bin/bash
# Honest TechEmpower-style benchmark of Wisp against the TFB reference sources.
# Usage: ./run.sh [build|verify|bench|all] [contender ...]     (run ON the benchmark host)
# Knobs (env): SERVER_CPUS=0-1 CLIENT_CPUS=2-3 DURATION=15 RUNS=3 CONTENDERS="..."
#
# Method follows TFB's toolset/wrk/{concurrency,pipeline}.sh and run-tests.py defaults:
#   primer 5 s c=8 -t8, sleep 5, warmup DURATION s at the max concurrency, sleep 5, then each level
#   (--latency -d DURATION --timeout 8, Connection: keep-alive, TFB's Accept headers).
#   json levels 16 32 64 128 256 512 (no pipelining); plaintext levels 256 1024 4096 16384 with
#   pipeline.lua at depth 16. We repeat every level RUNS times back to back.
set -u
ROOT=$(cd "$(dirname "$0")" && pwd)
SERVER_CPUS=${SERVER_CPUS:-0-1}
CLIENT_CPUS=${CLIENT_CPUS:-2-3}
DURATION=${DURATION:-15}
RUNS=${RUNS:-3}
WORKLOADS=${WORKLOADS:-"plaintext json"}   # a crashed server can be re-measured for one workload: WORKLOADS=json ./run.sh bench next
CONTENDERS=${CONTENDERS:-"wisp axum actix express fastify hono-node hono-bun sveltekit next wisp-uncapped"}
JSON_LEVELS="16 32 64 128 256 512"
PLAIN_LEVELS="256 1024 4096 16384"
PIPELINE=16
RAW=$ROOT/raw
export PATH=/opt/node/bin:/opt/bun/bin:$HOME/.cargo/bin:$PATH
ACCEPT_JSON='application/json,text/html;q=0.9,application/xhtml+xml;q=0.9,application/xml;q=0.8,*/*;q=0.7'
ACCEPT_PLAIN='text/plain,text/html;q=0.9,application/xhtml+xml;q=0.9,application/xml;q=0.8,*/*;q=0.7'
HOST=127.0.0.1
ulimit -n 1048576 2>/dev/null || ulimit -n 65535
mkdir -p "$RAW"

port_of() { case $1 in axum) echo 8000;; *) echo 8080;; esac; }

build() {
  [ "$1" = wisp-uncapped ] && return 0   # same binary as wisp
  cd "$ROOT/$1" || return 1
  case $1 in
    wisp) cargo build --release ;;
    axum|actix) RUSTFLAGS="-C target-cpu=native" cargo build --release ;;   # as TFB's axum/actix dockerfiles
    express|fastify) NODE_ENV=production npm install --no-audit --no-fund ;;
    hono-node) NODE_ENV=production npm install --no-audit --no-fund ;;
    hono-bun) bun install ;;
    sveltekit) npm install --no-audit --no-fund && npx vite build ;;
    next) npm install --no-audit --no-fund && npx next build ;;
  esac
}

launch() { local dir=$1 log=$2; shift 2; ( cd "$dir" && exec setsid "$@" ) >"$log" 2>&1 & PGID=$!; }

# Starts the contender pinned to SERVER_CPUS in its own process group (PGID).
start() {
  local c=$1 p; p=$(port_of "$c")
  local pin="taskset -c $SERVER_CPUS"
  mkdir -p "$RAW/$c"
  local log=$RAW/$c/server.log
  case $c in
    wisp)      launch "$ROOT/wisp" "$log" env PORT=$p $pin ./target/release/tfb-wisp ;;
    wisp-uncapped) launch "$ROOT/wisp" "$log" env PORT=$p WISP_MAX_CONNS=0 $pin ./target/release/tfb-wisp ;;   # supplementary: lifts the default 10000-connection cap
    axum)      launch "$ROOT/axum" "$log" $pin ./target/release/axum ;;
    actix)     launch "$ROOT/actix" "$log" $pin ./target/release/tfb-web ;;
    express)   launch "$ROOT/express" "$log" env NODE_ENV=production $pin node app.js ;;
    fastify)   launch "$ROOT/fastify" "$log" env NODE_ENV=production $pin node app.js ;;
    hono-node) launch "$ROOT/hono-node" "$log" env NODE_ENV=production $pin npm start ;;
    hono-bun)  launch "$ROOT/hono-bun" "$log" env PORT=$p $pin bash -c 'for i in $(seq $(nproc)); do bun server.js & done; wait' ;;
    sveltekit) launch "$ROOT/sveltekit" "$log" env NODE_ENV=production PORT=$p $pin node cluster.js ;;
    next)      launch "$ROOT/next" "$log" env NODE_ENV=production PORT=$p HOSTNAME=0.0.0.0 $pin node cluster.js ;;
  esac
  for i in $(seq 150); do curl -s -o /dev/null "http://$HOST:$p/plaintext" && return 0; sleep 0.2; done
  return 1
}

stop() {
  if [ -n "${PGID:-}" ]; then kill -- -"$PGID" 2>/dev/null; sleep 1; kill -9 -- -"$PGID" 2>/dev/null; fi
  PGID=
  sleep 2
}

# TFB's verification: 200, content type, Content-Length or Transfer-Encoding, Server, Date
# (and that it changes), no gzip, exact body, plus 16-deep pipelining.
verify() {
  local c=$1 p; p=$(port_of "$c"); local f=$RAW/$c/verify.txt; : >"$f"; local ok=0
  for t in json plaintext; do
    local want_ct want_body acc
    if [ $t = json ]; then want_ct=application/json; want_body='{"message":"Hello, World!"}'; acc=$ACCEPT_JSON
    else want_ct=text/plain; want_body='Hello, World!'; acc=$ACCEPT_PLAIN; fi
    echo "=== GET /$t" >>"$f"
    curl -s -i --max-time 5 -H "Accept: $acc" "http://$HOST:$p/$t" >"$f.tmp"; cat "$f.tmp" >>"$f"; echo >>"$f"
    local head body
    head=$(sed -n '1,/^\r$/p' "$f.tmp" | tr -d '\r'); body=$(sed '1,/^\r$/d' "$f.tmp")
    echo "$head" | head -1 | grep -q ' 200' || { echo "FAIL $t status" >>"$f"; ok=1; }
    echo "$head" | grep -i '^content-type:' | grep -qi "^content-type: *$want_ct" || { echo "FAIL $t content-type" >>"$f"; ok=1; }
    echo "$head" | grep -iqE '^(content-length|transfer-encoding):' || { echo "FAIL $t length" >>"$f"; ok=1; }
    echo "$head" | grep -iq '^server:' || { echo "FAIL $t Server" >>"$f"; ok=1; }
    echo "$head" | grep -iq '^date:' || { echo "FAIL $t Date" >>"$f"; ok=1; }
    echo "$head" | grep -iq '^content-encoding:' && { echo "FAIL $t gzip" >>"$f"; ok=1; }
    [ "$body" = "$want_body" ] || { echo "FAIL $t body [$body]" >>"$f"; ok=1; }
  done
  local d1 d2
  d1=$(curl -si "http://$HOST:$p/plaintext" | grep -i '^date:'); sleep 1.2; d2=$(curl -si "http://$HOST:$p/plaintext" | grep -i '^date:')
  [ "$d1" != "$d2" ] || { echo "FAIL Date not updating" >>"$f"; ok=1; }
  local n
  n=$(bash -c "exec 3<>/dev/tcp/$HOST/$p; for i in \$(seq 16); do printf 'GET /plaintext HTTP/1.1\r\nHost: x\r\nAccept: %s\r\n\r\n' '$ACCEPT_PLAIN' >&3; done; timeout 1 cat <&3" | grep -c '^Hello, World!')
  echo "pipelined 16 -> $n bodies" >>"$f"; [ "$n" = 16 ] || { echo "FAIL pipelining" >>"$f"; ok=1; }
  echo "processes: $(pgrep -g "$PGID" | wc -l), threads: $(ps -L -o tid= -g "$PGID" 2>/dev/null | wc -l), affinity: $(taskset -cp "$PGID" | sed 's/.*: //')" >>"$f"
  rm -f "$f.tmp"; return $ok
}

cpusnap() { awk '/^cpu /{print $2,$3,$4,$5,$6,$7,$8,$9}' /proc/stat; }   # user nice sys idle iowait irq softirq steal

# Nothing else may run on this VM while we measure: wait for < 2% busy over 5 s on all cores.
wait_quiet() {
  local a b tot busy k
  for i in $(seq 180); do
    a=($(cpusnap)); sleep 5; b=($(cpusnap))
    tot=0; for k in 0 1 2 3 4 5 6 7; do tot=$((tot + b[k] - a[k])); done
    busy=$((tot - (b[3]-a[3]) - (b[4]-a[4])))
    [ $((busy * 1000 / tot)) -lt 20 ] && return 0
    echo "waiting for the host to go quiet ($((busy*100/tot))% busy)"
  done
  echo "WARNING: host never went quiet"; return 1
}

# CPU ticks (user+sys) used so far by the server's process group.
group_ticks() { local t=0 p x; for p in $(pgrep -g "$PGID"); do x=$(awk '{sub(/^.*\) /,""); print $12+$13}' /proc/$p/stat 2>/dev/null); t=$((t + ${x:-0})); done; echo $t; }
# CPU ticks (100 Hz) used by children this shell has waited for (wrk).
child_ticks() { times >"$RAW/.times"; CT=$(sed -n 2p "$RAW/.times" | awk '{ split($1,a,/[ms]/); split($2,b,/[ms]/); printf "%d", (a[1]*60+a[2]+b[1]*60+b[2])*100 }'); }

# wrk_once <path> <accept> <connections> <outfile> <seconds> [pipeline depth]
# Also records "foreign": CPU used by anything but the server and wrk (user+sys+irq ticks, as % of all cores),
# so a run that a neighbour on this shared VM disturbed is visible (and is repeated by wrk_run).
wrk_once() {
  local path=$1 acc=$2 c=$3 out=$4 secs=$5 pl=${6:-} t ct k
  ct=$(taskset -c "$CLIENT_CPUS" nproc); t=$(( c > ct ? ct : c ))   # TFB: -t min(c, nproc), nproc = client cores
  local a=($(cpusnap)) g0 g1 w0 w1 b ncpu=$(nproc --all)
  g0=$(group_ticks); child_ticks; w0=$CT
  {
    echo "# loadavg-before: $(cut -d' ' -f1-3 /proc/loadavg)"
    echo "# cmd: taskset -c $CLIENT_CPUS wrk -H 'Host: $HOST' -H 'Accept: $acc' -H 'Connection: keep-alive' --latency -d $secs -c $c --timeout 8 -t $t http://$HOST:$PORT$path ${pl:+-s pipeline.lua -- $pl}"
    local s0=$SECONDS
    taskset -c "$CLIENT_CPUS" wrk -H "Host: $HOST" -H "Accept: $acc" -H 'Connection: keep-alive' --latency -d "$secs" -c "$c" --timeout 8 -t "$t" "http://$HOST:$PORT$path" ${pl:+-s "$ROOT/pipeline.lua" -- $pl}
    b=($(cpusnap)); g1=$(group_ticks); child_ticks; w1=$CT
    local used=$(( (b[0]+b[1]+b[2]+b[5]+b[6]) - (a[0]+a[1]+a[2]+a[5]+a[6]) )) mine=$(( (g1-g0) + (w1-w0) )) wall=$(( SECONDS - s0 ))
    echo "# cpu-delta user nice sys idle iowait irq softirq steal: $(for k in 0 1 2 3 4 5 6 7; do printf '%s ' $((b[k]-a[k])); done)"
    echo "# foreign-pct: $(awk -v u=$used -v m=$mine -v n=$ncpu -v w=$((wall>0?wall:1)) 'BEGIN{f=(u-m)*100/(n*w*100); if (f<0) f=0; printf "%.1f", f}')  (user+sys+irq ticks $used, server+wrk $mine)"
    echo "# top-after: $(ps -eo pcpu,comm --sort=-pcpu | sed -n '2,4p' | tr -s ' ' | tr '\n' ';')"
    # A server that stopped answering during the run is a stall, whatever wrk counted.
    curl -s -o /dev/null --max-time 5 "http://$HOST:$PORT/plaintext" && echo "# alive-after: yes" || echo "# alive-after: no"
  } >"$out" 2>&1
}

# A run another tenant of this VM disturbed (foreign CPU > 5%) is kept as <out>.tainted<N> and repeated.
wrk_run() {
  local out=$4 n f
  for n in 1 2 3 4; do
    wrk_once "$@"
    f=$(sed -n 's/^# foreign-pct: \([0-9.]*\).*/\1/p' "$out")
    awk -v f="${f:-0}" 'BEGIN{exit !(f > 5)}' || return 0
    [ $n = 4 ] && { echo "# NOTE: foreign CPU stayed above 5% on every attempt" >>"$out"; return 0; }
    mv "$out" "$out.tainted$n"; wait_quiet
  done
}

bench_one() {
  local c=$1 w levels acc pl maxc l r; PORT=$(port_of "$c"); mkdir -p "$RAW/$c"
  wait_quiet
  start "$c" || { echo "OMIT $c: did not start (raw/$c/server.log)" | tee "$RAW/$c/OMITTED"; stop; return; }
  if ! verify "$c"; then echo "OMIT $c: verification failed (raw/$c/verify.txt)" | tee "$RAW/$c/OMITTED"; stop; return; fi
  for w in $WORKLOADS; do
    pl=""
    if [ $w = json ]; then levels=$JSON_LEVELS; acc=$ACCEPT_JSON; else levels=$PLAIN_LEVELS; acc=$ACCEPT_PLAIN; pl=$PIPELINE; fi
    maxc=${levels##* }
    wrk_run "/$w" "$acc" 8 "$RAW/$c/$w-primer.txt" 5 $pl; sleep 5
    wrk_run "/$w" "$acc" "$maxc" "$RAW/$c/$w-warmup.txt" "$DURATION" $pl; sleep 5
    for l in $levels; do
      for r in $(seq "$RUNS"); do
        wrk_run "/$w" "$acc" "$l" "$RAW/$c/$w-c$l-run$r.txt" "$DURATION" $pl; sleep 2
      done
    done
  done
  stop
}

record_env() {
  {
    echo "date: $(date -u +%FT%TZ)"
    echo "cpu: $(lscpu | grep -m1 'Model name' | sed 's/.*: *//')"
    echo "cpus: $(nproc --all)  server_cpus=$SERVER_CPUS client_cpus=$CLIENT_CPUS duration=$DURATION runs=$RUNS"
    grep MemTotal /proc/meminfo; echo "kernel: $(uname -r)"; grep PRETTY /etc/os-release
    echo "wrk: $(wrk --version 2>&1 | head -1)"; echo "rustc: $(rustc -V)"; echo "node: $(node -v)"; echo "bun: $(bun -v)"
    echo "somaxconn: $(cat /proc/sys/net/core/somaxconn) tcp_max_syn_backlog: $(cat /proc/sys/net/ipv4/tcp_max_syn_backlog) ulimit-n: $(ulimit -n)"
    echo "governor: $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>&1 | head -1)"
    echo "wisp rev: $(cat "$ROOT/WISP_REV" 2>/dev/null)"
    echo "crates (Cargo.lock):"
    grep -A1 -E '^name = "(axum|hyper|tokio|actix-web|actix-http|mimalloc|snmalloc-rs)"' "$ROOT/axum/Cargo.lock" "$ROOT/actix/Cargo.lock" | grep -E 'name|version' | paste - - | sed 's/ \+/ /g'
    for d in express fastify hono-node sveltekit next; do echo "$d: $(cd "$ROOT/$d" && npm ls --depth=0 2>/dev/null | tail -n +2 | tr '\n' ' ')"; done
    echo "hono-bun: $(cd "$ROOT/hono-bun" && bun pm ls 2>/dev/null | tr '\n' ' ')"
  } >"$RAW/env.txt" 2>&1
}

stage=${1:-all}; shift 2>/dev/null
[ $# -gt 0 ] && CONTENDERS="$*"
cd "$ROOT"
case $stage in
  build|all)
    for c in $CONTENDERS; do
      echo "== build $c"; mkdir -p "$RAW/$c"; rm -f "$RAW/$c/OMITTED"
      build "$c" >"$RAW/$c/build.log" 2>&1 || echo "OMIT $c: build failed (raw/$c/build.log)" | tee "$RAW/$c/OMITTED"
    done ;;&
  verify)
    for c in $CONTENDERS; do
      if start "$c" && verify "$c"; then echo "$c ok"; else echo "$c FAIL"; fi
      stop
    done ;;
  noise)   # how far apart is the SAME binary, measured at different moments? (wisp vs wisp-uncapped, interleaved)
    mkdir -p "$RAW/noise"; wait_quiet
    for r in 1 2 3; do
      for c in wisp wisp-uncapped; do
        PORT=8080; start "$c" || continue
        wrk_run /json "$ACCEPT_JSON" 64 "$RAW/noise/$c-json-warm-r$r.txt" 5
        for l in 16 256; do wrk_run /json "$ACCEPT_JSON" $l "$RAW/noise/$c-json-c$l-r$r.txt" "$DURATION"; sleep 2; done
        stop
      done
    done ;;
  bench|all)
    [ -e "$RAW/env.txt" ] && [ "$WORKLOADS" != "plaintext json" ] || record_env
    for c in $CONTENDERS; do
      [ -e "$RAW/$c/OMITTED" ] && continue
      echo "== bench $c $(date +%T)"; bench_one "$c"
    done ;;
esac
