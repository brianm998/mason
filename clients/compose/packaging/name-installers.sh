#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Rename Compose's installers to the names in docs/RELEASING.md.
#   name-installers.sh VERSION [SRC_DIR] [OUT_DIR]
# SRC_DIR defaults to desktop/build/compose/binaries, OUT_DIR to dist.
set -euo pipefail
cd "$(dirname "$0")/.."
ver=${1:?usage: name-installers.sh VERSION [SRC_DIR] [OUT_DIR]}
src=${2:-desktop/build/compose/binaries}; out=${3:-dist}
mkdir -p "$out"; n=0
for f in $(find "$src" -name '*.deb' -o -name '*.msi'); do
  case $f in
    *.deb) arch=$(dpkg --print-architecture 2>/dev/null || echo amd64)
           to="mason-monitor_${ver}_${arch}.deb" ;;
    *.msi) to="MASON-Monitor-${ver}-windows.msi" ;;
  esac
  cp "$f" "$out/$to"; echo "$out/$to"; n=$((n+1))
done
[ "$n" -gt 0 ] || { echo "no installers under $src" >&2; exit 1; }
