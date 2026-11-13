#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Write DIR/SHA256SUMS ("<hash>  <file name>", names only) for every asset in DIR. SHA256SUMS
# and its signature are not assets. Usage: checksums.sh DIR
set -euo pipefail
dir="${1:-}"
[ -d "$dir" ] || { echo "usage: checksums.sh DIR" >&2; exit 2; }
cd "$dir"
if command -v sha256sum >/dev/null 2>&1; then sum() { sha256sum "$1"; }
elif command -v shasum >/dev/null 2>&1; then sum() { shasum -a 256 "$1"; }
else echo "checksums: need sha256sum or shasum" >&2; exit 1; fi
tmp=$(mktemp "${TMPDIR:-/tmp}/mason-sums.XXXXXX")
trap 'rm -f "$tmp"' EXIT
n=0
for f in $(ls -1 | LC_ALL=C sort); do
  [ -f "$f" ] || continue
  case "$f" in SHA256SUMS|SHA256SUMS.*) continue ;; esac
  sum "$f" >> "$tmp"; n=$((n+1))
done
[ "$n" -gt 0 ] || { echo "checksums: no assets in $dir" >&2; exit 1; }
cat "$tmp" > SHA256SUMS
echo "SHA256SUMS: $n assets"
