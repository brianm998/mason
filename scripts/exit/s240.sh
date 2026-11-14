#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s240 exit test: macos-pkg.sh with no signing env builds the universal .pkg; its payload lists the
# four bins and askpass and the extracted mason is x86_64 + arm64. Notarization is not tested here.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
if [ "$(uname -s)" != Darwin ]; then echo "s240: skipped, not macOS"; exit 0; fi
for t in aarch64-apple-darwin x86_64-apple-darwin; do
  rustup target list --installed 2>/dev/null | grep -qx "$t" || { echo "s240: skipped, rust target $t not installed"; exit 0; }
done
bash -n scripts/package/notarize.sh
(unset SIGN_APP SIGN_PKG; bash scripts/package/notarize.sh /nonexistent 2>&1 | grep -q "no such file") || { echo "FAILED: notarize.sh missing-file check" >&2; exit 1; }
d=$(mktemp -d /tmp/mason-s240.XXXXXX); trap 'rm -rf "$d"' EXIT
(unset SIGN_APP SIGN_PKG; bash scripts/package/macos-pkg.sh --out "$d/out")
pkg=$(ls "$d"/out/mason-*-macos.pkg)
files=$(/usr/sbin/pkgutil --payload-files "$pkg")
for f in mason masond step-host mason-mcp mason-askpass.sh; do
  echo "$files" | grep -q "usr/local/bin/$f\$" || { echo "FAILED: payload lacks $f" >&2; exit 1; }
done
/usr/sbin/pkgutil --expand-full "$pkg" "$d/x"
archs=$(/usr/bin/lipo -archs "$(find "$d/x" -path '*usr/local/bin/mason' | head -1)")
for a in x86_64 arm64; do
  echo "$archs" | grep -qw "$a" || { echo "FAILED: mason archs '$archs' lack $a" >&2; exit 1; }
done
echo "s240: ok ($archs)"
