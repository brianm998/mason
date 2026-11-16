#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s448 exit test: monitor_inplace.rs has no ungated std::os::unix use; checks Windows if a target is installed.
set -uo pipefail
cd "$(dirname "$0")/../.."
f=crates/daemon/src/monitor_inplace.rs
bad=$(awk '
  /^[[:space:]]*#\[cfg\(.*unix/ {gated=NR}
  /std::os::unix/ { if (!gated) print NR": "$0 }' "$f")
if [ -n "$bad" ]; then echo "ungated unix use:"; echo "$bad"; exit 1; fi
echo "no ungated std::os::unix use"
for t in x86_64-pc-windows-msvc x86_64-pc-windows-gnu; do
  if rustup target list --installed 2>/dev/null | grep -qx "$t"; then
    out=$(mktemp /tmp/s448.XXXXXX)
    if cargo check -q -p mason-daemon --all-targets --target "$t" >"$out" 2>&1; then
      echo "windows check ($t) green"; rm -f "$out"; exit 0
    fi
    if grep -q "error occurred in cc-rs" "$out" && ! grep -q "^error\[E" "$out"; then
      echo "windows check ($t) cannot build C dependencies on this Mac (no Windows SDK); not verified here"
      rm -f "$out"; exit 0
    fi
    tail -40 "$out"; rm -f "$out"; exit 1
  fi
done
echo "no Windows target installed: the windows CI job is the only guard (not verified here)"
