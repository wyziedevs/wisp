#!/bin/bash
# per host: up to 4 attempts, each waits for steal <=3% over 30 s and holds the bench flock for that attempt only (others interleave).
# Stops at the first fully valid attempt (kept). Otherwise the LAST attempt is kept, marked invalid, not ranked. All attempts recorded.
export PATH=$HOME/.cargo/bin:$HOME/.local/node/bin:$HOME/.local/bun/bin:$PATH
cd /root/rp/bench/rank
quiet() { while :; do m=$(vmstat 5 7 | awk 'NR>3{s+=$17;n++} END{printf "%d", s/n}'); [ "$m" -le 3 ] && return; echo "steal $m, waiting $(date +%T)"; sleep 120; done; }
for h in ${HOSTS:-workerd deno node bun}; do
  files="results/$h.try0.json"; done_ok=0
  for t in 1 2 3 4; do
    if [ ! -f results/$h.try$t.json ]; then
      quiet
      echo "== $h try $t $(date)"
      HOSTS=$h flock /tmp/wisp-bench.lock sh run.sh >/dev/null 2>&1
      cp results/$h.json results/$h.try$t.json; cp results/$h.vmstat results/$h.try$t.vmstat
    fi
    files="$files results/$h.try$t.json"
    [ "$(node score.mjs $h)" -eq 0 ] && break
  done
  node finalize.mjs $h $files
done
node report.mjs > results/report.md
echo ALLDONE $(date)
