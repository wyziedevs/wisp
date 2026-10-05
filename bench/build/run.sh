#!/usr/bin/env sh
# Build-time and dev-loop numbers for a Wisp app (bench/README.md, "Build times").
#   bench/build/run.sh <dir> [routes]   makes <dir> with `wisp new`, adds
#   [routes] generated routes (gen.sh), then times cold and incremental
#   builds. REL=1 adds a cold release build. Uses the `wisp` on PATH, or $WISP.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
wisp=${WISP:-wisp}
dir=$1
n=${2:-0}
[ -d "$dir" ] || "$wisp" new "$dir" >/dev/null
[ "$n" -gt 0 ] && sh "$here/gen.sh" "$dir" "$n"
cd "$dir"
name=$(sed -n 's/^name = "\(.*\)"/\1/p' Cargo.toml | head -1)
t() {
  s=$(date +%s.%N)
  "$@" >/dev/null 2>&1
  e=$(date +%s.%N)
  awk "BEGIN{printf \"%.1fs\", $e - $s}"
}
cargo clean -q
echo "cold build (debug):    $(t cargo build -q)"
echo "no-op build:           $(t cargo build -q)"
echo "<!-- e -->" >>src/routes/+page.wisp
echo "template edit build:   $(t cargo build -q)"
echo "// e" >>src/main.rs
echo "Rust edit build:       $(t cargo build -q)"
mkdir -p src/routes/zz-new && echo "<p>new</p>" >src/routes/zz-new/+page.wisp
echo "new route build:       $(t cargo build -q)"
rm -rf src/routes/zz-new
echo "// f" >>src/main.rs
echo "wisp check --rust:     $(t "$wisp" check --rust)"
out=$(ls -t target/debug/build/"$name"-*/out/wisp.rs | head -1)
echo "generated wisp.rs:     $(wc -l <"$out") lines, $(wc -c <"$out") bytes"
exe=target/debug/$name
[ -f "$exe" ] || exe=$exe.exe
echo "debug binary:          $(wc -c <"$exe") bytes"
if [ -n "${REL:-}" ]; then
  echo "cold release build:    $(t cargo build -q --release)"
  exe=target/release/$name
  [ -f "$exe" ] || exe=$exe.exe
  echo "release binary:        $(wc -c <"$exe") bytes"
fi
