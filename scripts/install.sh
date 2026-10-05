#!/bin/sh
# Installs the `wisp` command from the GitHub release: picks the archive for
# this OS and CPU, checks its SHA-256, puts `wisp` in $WISP_INSTALL_DIR
# (default ~/.local/bin). WISP_VERSION=v0.1.0 picks a version (default: latest).
# Meant to be served at https://wispweb.dev/install (not yet).
set -eu

repo="wyziedevs/wisp"
dir="${WISP_INSTALL_DIR:-$HOME/.local/bin}"
version="${WISP_VERSION:-latest}"

case "$(uname -s)" in
  Linux) os=unknown-linux-musl ;;
  Darwin) os=apple-darwin ;;
  *) echo "wisp: no prebuilt binary for $(uname -s); run: cargo install wisp-web" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) cpu=x86_64 ;;
  arm64 | aarch64) cpu=aarch64 ;;
  *) echo "wisp: no prebuilt binary for $(uname -m); run: cargo install wisp-web" >&2; exit 1 ;;
esac
name="wisp-$cpu-$os"
if [ "$version" = latest ]; then
  base="https://github.com/$repo/releases/latest/download"
else
  base="https://github.com/$repo/releases/download/$version"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
fetch() {
  if command -v curl >/dev/null; then curl -fsSL "$1" -o "$2"; else wget -qO "$2" "$1"; fi
}
fetch "$base/$name.tar.gz" "$tmp/$name.tar.gz"
fetch "$base/$name.tar.gz.sha256" "$tmp/$name.tar.gz.sha256"

want="$(cut -d' ' -f1 < "$tmp/$name.tar.gz.sha256")"
if command -v sha256sum >/dev/null; then
  got="$(sha256sum "$tmp/$name.tar.gz" | cut -d' ' -f1)"
else
  got="$(shasum -a 256 "$tmp/$name.tar.gz" | cut -d' ' -f1)"
fi
if [ "$want" != "$got" ]; then
  echo "wisp: checksum mismatch for $name.tar.gz (want $want, got $got)" >&2
  exit 1
fi

tar xzf "$tmp/$name.tar.gz" -C "$tmp"
mkdir -p "$dir"
mv "$tmp/$name/wisp" "$dir/wisp"
chmod +x "$dir/wisp"
echo "wisp installed to $dir/wisp"
case ":$PATH:" in
  *":$dir:"*) ;;
  *) echo "Add $dir to your PATH." ;;
esac
