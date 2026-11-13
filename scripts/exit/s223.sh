#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s223 exit test: build the host's tarball, checksum it, install it with scripts/install.sh from
# a file:// release and check `mason --version`; check the .deb with dpkg-deb when available.
set -euo pipefail
cd "$(dirname "$0")/../.."

ver=$(awk '/^\[workspace\.package\]/{f=1;next} /^\[/{f=0} f && /^version *=/{gsub(/[" ]/,"",$0); sub(/^version=/,"",$0); print; exit}' Cargo.toml)
[ -n "$ver" ] || { echo "FAILED: no workspace version" >&2; exit 1; }

arch=$(uname -m); case "$arch" in arm64|aarch64) arch=aarch64 ;; *) arch=x86_64 ;; esac
case "$(uname -s)" in
  Darwin) target="$arch-apple-darwin" ;;
  Linux) target="$arch-unknown-linux-gnu" ;;
  *) echo "s223: skipped, no tarball target for $(uname -s)"; exit 0 ;;
esac

tmp=$(mktemp -d "${TMPDIR:-/tmp}/s223.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
rel="$tmp/release"; mkdir -p "$rel"

echo "== tarball for $target"
bash scripts/package/tarball.sh "$target" --out "$rel"
name="mason-$ver-$target"
[ -f "$rel/$name.tar.gz" ] || { echo "FAILED: $name.tar.gz missing" >&2; exit 1; }
listing=$(tar -tzf "$rel/$name.tar.gz")
for f in mason masond step-host mason-mcp mason-askpass.sh LICENSE LICENSE-ADDITIONAL-PERMISSION README.md; do
  echo "$listing" | grep -qx "$name/$f" || { echo "FAILED: tarball lacks $name/$f" >&2; exit 1; }
done

echo "== checksums"
bash scripts/package/checksums.sh "$rel"
grep -q "  $name.tar.gz\$" "$rel/SHA256SUMS" || { echo "FAILED: SHA256SUMS has no $name.tar.gz" >&2; exit 1; }
if grep -q '/' "$rel/SHA256SUMS"; then echo "FAILED: SHA256SUMS holds paths" >&2; exit 1; fi

echo "== install.sh from the fake release"
bash scripts/install.sh --version "$ver" --base-url "file://$rel" --prefix "$tmp/prefix"
got=$("$tmp/prefix/bin/mason" --version)
case "$got" in "mason $ver "*) echo "$got" ;; *) echo "FAILED: mason --version said '$got', want 'mason $ver ...'" >&2; exit 1 ;; esac

echo "== .deb"
if command -v dpkg-deb >/dev/null 2>&1 && [ "$(uname -s)" = Linux ]; then
  case "$target" in aarch64*) darch=arm64 ;; *) darch=amd64 ;; esac
  MASON_PACKAGE_NO_BUILD=1 bash scripts/package/deb.sh "$target" --out "$rel"
  deb="$rel/mason_${ver}_${darch}.deb"
  info=$(dpkg-deb --info "$deb")
  echo "$info" | grep -q "Depends: git" || { echo "FAILED: no Depends: git" >&2; exit 1; }
  contents=$(dpkg-deb --contents "$deb")
  for f in mason masond step-host mason-mcp mason-askpass.sh; do
    echo "$contents" | grep -q "usr/bin/$f\$" || { echo "FAILED: deb lacks /usr/bin/$f" >&2; exit 1; }
  done
  echo "$contents" | grep -q "usr/share/doc/mason/copyright" || { echo "FAILED: deb lacks copyright" >&2; exit 1; }
else
  echo "skipped: dpkg-deb not available here; the Linux release job installs the .deb for real"
fi
echo "s223 OK"
