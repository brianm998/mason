#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Build mason-<ver>-<target>.tar.gz: a top-level folder mason-<ver>-<target>/ holding mason,
# masond, step-host, mason-mcp, mason-askpass.sh, LICENSE, LICENSE-ADDITIONAL-PERMISSION and
# README.md. Usage: tarball.sh <target> [--out DIR]   (default DIR: dist)
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
[ -n "$target" ] || pkg_die "usage: tarball.sh <target> [--out DIR]"
case "$target" in
  aarch64-apple-darwin|x86_64-apple-darwin|x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu) ;;
  *) pkg_die "no tarball for $target (see docs/RELEASING.md)" ;;
esac

ver=$(pkg_version)
name="mason-$ver-$target"
work=$(mktemp -d "${TMPDIR:-/tmp}/mason-tarball.XXXXXX")
trap 'rm -rf "$work"' EXIT
pkg_build "$target" "$work/$name"
cp "$ROOT/LICENSE" "$ROOT/LICENSE-ADDITIONAL-PERMISSION" "$ROOT/README.md" "$work/$name/"
mkdir -p "$out"
out=$(cd "$out" && pwd)
tar -C "$work" -czf "$out/$name.tar.gz" "$name"
echo "$out/$name.tar.gz"
