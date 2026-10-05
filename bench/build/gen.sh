#!/usr/bin/env sh
# Adds N generated routes to a `wisp new` app: bench/build/gen.sh <app> <n>
# Each route has a +page.rs load and a +page.wisp with a loop, an if and holes.
set -eu
app=$1
n=$2
i=0
while [ "$i" -lt "$n" ]; do
  d="$app/src/routes/gen/r$i"
  mkdir -p "$d"
  cat >"$d/+page.rs" <<EOF
struct Data {
    title: String,
    items: Vec<u32>,
    flag: bool,
}

fn load(cx: &mut Cx) -> Data {
    let _ = cx;
    Data { title: "Route $i".into(), items: (0..$i % 7 + 1).collect(), flag: $i % 2 == 0 }
}
EOF
  cat >"$d/+page.wisp" <<EOF
<head><title>{title}</title></head>
<h1>{title}</h1>
{#if flag}<p class="even">even route $i</p>{:else}<p>odd</p>{/if}
<ul>
{#each items as item, k}
  <li data-k="{k}">item {item} of <a href="/gen/r$i">{title}</a></li>
{/each}
</ul>
EOF
  i=$((i + 1))
done
