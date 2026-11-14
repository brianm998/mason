#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Build mason-<ver>-macos.pkg: universal (arm64 + x86_64) mason, masond, step-host, mason-mcp
# and mason-askpass.sh, installed to /usr/local/bin. Usage: macos-pkg.sh [--out DIR] (default: dist)
# Env: SIGN_APP  Developer ID Application identity (hardened runtime + timestamp); else ad hoc.
#      SIGN_PKG  Developer ID Installer identity for productsign; else the .pkg is unsigned.
#      MASON_PACKAGE_NO_BUILD=1 reuses earlier release builds (see lib.sh).
# Notarize afterwards with notarize.sh. Certificates are read only from the keychain/environment.
set -euo pipefail
. "$(dirname "$0")/lib.sh"

out="$ROOT/dist"
while [ $# -gt 0 ]; do
  case "$1" in
    --out) [ $# -ge 2 ] || pkg_die "--out needs a value"; out=$2; shift 2 ;;
    *) pkg_die "usage: macos-pkg.sh [--out DIR]" ;;
  esac
done
[ "$(uname -s)" = Darwin ] || pkg_die "macos-pkg.sh needs macOS (lipo, pkgbuild)"
LIPO=/usr/bin/lipo   # MacPorts' lipo comes first on some PATHs and rejects newer flags

ver=$(pkg_version)
work=$(mktemp -d "${TMPDIR:-/tmp}/mason-macos-pkg.XXXXXX")
trap 'rm -rf "$work"' EXIT
root="$work/root/usr/local/bin"
mkdir -p "$root"

pkg_build aarch64-apple-darwin "$work/arm64"
pkg_build x86_64-apple-darwin "$work/x86_64"

for b in $BINS; do
  "$LIPO" -create "$work/arm64/$b" "$work/x86_64/$b" -output "$root/$b" || pkg_die "lipo failed for $b"
  chmod 755 "$root/$b"
  if [ -n "${SIGN_APP:-}" ]; then
    codesign --force --options runtime --timestamp -s "$SIGN_APP" "$root/$b" || pkg_die "codesign failed for $b"
  else
    codesign --force -s - "$root/$b" || pkg_die "ad-hoc codesign failed for $b"
  fi
done
cp "$ROOT/packaging/mason-askpass.sh" "$root/mason-askpass.sh"
chmod 755 "$root/mason-askpass.sh"

mkdir -p "$out"
out=$(cd "$out" && pwd)
final="$out/mason-$ver-macos.pkg"
rm -f "$final"
if [ -n "${SIGN_PKG:-}" ]; then
  /usr/bin/pkgbuild --root "$work/root" --identifier com.mason.cli --version "$ver" \
    --install-location / "$work/unsigned.pkg" >/dev/null || pkg_die "pkgbuild failed"
  /usr/bin/productsign --timestamp --sign "$SIGN_PKG" "$work/unsigned.pkg" "$final" >/dev/null \
    || pkg_die "productsign failed"
else
  /usr/bin/pkgbuild --root "$work/root" --identifier com.mason.cli --version "$ver" \
    --install-location / "$final" >/dev/null || pkg_die "pkgbuild failed"
fi
echo "$final"
