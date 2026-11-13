#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Build mason_<ver>_<arch>.deb with dpkg-deb: the four binaries and mason-askpass.sh in
# /usr/bin, Depends: git, a copyright file, and a postinst that only prints the next step.
# Usage: deb.sh <target> [--out DIR]   (x86_64-unknown-linux-gnu or aarch64-unknown-linux-gnu)
set -euo pipefail
. "$(dirname "$0")/lib.sh"

target=""; out="$ROOT/dist"
while [ $# -gt 0 ]; do
  case "$1" in
    --out) [ $# -ge 2 ] || pkg_die "--out needs a value"; out=$2; shift 2 ;;
    -*) pkg_die "unknown option $1" ;;
    *) [ -z "$target" ] || pkg_die "one target only"; target=$1; shift ;;
  esac
done
[ -n "$target" ] || pkg_die "usage: deb.sh <target> [--out DIR]"
case "$target" in
  x86_64-unknown-linux-gnu) arch=amd64 ;;
  aarch64-unknown-linux-gnu) arch=arm64 ;;
  *) pkg_die "no .deb for $target" ;;
esac
command -v dpkg-deb >/dev/null 2>&1 || pkg_die "dpkg-deb not found; build the .deb on Linux"

ver=$(pkg_version)
work=$(mktemp -d "${TMPDIR:-/tmp}/mason-deb.XXXXXX")
trap 'rm -rf "$work"' EXIT
pkg="$work/pkg"
mkdir -p "$pkg/usr/bin" "$pkg/usr/share/doc/mason" "$pkg/DEBIAN"
pkg_build "$target" "$pkg/usr/bin"

cat > "$pkg/usr/share/doc/mason/copyright" <<COPY
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: mason
Source: https://github.com/brianm998/mason

Files: *
Copyright: the MASON authors
License: GPL-3.0-or-later with additional permission
 The full text is in /usr/share/doc/mason/LICENSE and, for the additional permission,
 /usr/share/doc/mason/LICENSE-ADDITIONAL-PERMISSION. On Debian systems the GPL-3 text is also
 in /usr/share/common-licenses/GPL-3.
COPY
cp "$ROOT/LICENSE" "$ROOT/LICENSE-ADDITIONAL-PERMISSION" "$pkg/usr/share/doc/mason/"

size=$(du -sk "$pkg/usr" | cut -f1)
cat > "$pkg/DEBIAN/control" <<CTL
Package: mason
Version: $ver
Architecture: $arch
Maintainer: MASON authors <noreply@github.com>
Depends: git
Section: devel
Priority: optional
Installed-Size: $size
Homepage: https://github.com/brianm998/mason
Description: pipeline that takes tasks from a tracker to landed code with LLM agents
 The mason command-line client, the masond daemon, step-host and mason-mcp.
CTL

cat > "$pkg/DEBIAN/postinst" <<'POST'
#!/bin/sh
# Prints the next step only. It never starts, enables or configures anything.
set -e
if [ "$1" = configure ]; then
  echo "mason is installed. Next: run  mason service install  to set up the background daemon."
fi
exit 0
POST
chmod 755 "$pkg/DEBIAN/postinst"
find "$pkg" -type d -exec chmod 755 {} +

mkdir -p "$out"
out=$(cd "$out" && pwd)
dpkg-deb --root-owner-group -Zxz --build "$pkg" "$out/mason_${ver}_${arch}.deb" >&2
echo "$out/mason_${ver}_${arch}.deb"
