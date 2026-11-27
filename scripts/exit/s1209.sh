#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 1209 exit test: s06's host search survives a host.kv removed between its test and its read.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
grep -q "2>/dev/null || true)" <(grep "host_pid=//p" scripts/exit/s06.sh)
# Take the real discovery loop out of s06.sh so this checks what the script runs.
loop=$(sed -n '/^host_pid=""$/,/^done$/p' scripts/exit/s06.sh)
[ -n "$loop" ]
root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT
mkdir -p "$root/d/h/t0r1-1" "$root/d/h/t0r1-2"
echo "host_pid=999999" > "$root/d/h/t0r1-1/host.kv"
echo "host_pid=$$" > "$root/d/h/t0r1-2/host.kv"
# Shim: the first read of a host.kv finds it gone, as the CI failure did.
gone=0
sed() {
  if [ "$gone" = 0 ]; then gone=1; rm -f "${@: -1}"; fi
  command sed "$@"
}
eval "$loop"
[ "$host_pid" = "$$" ] || { echo "picked '$host_pid', wanted $$" >&2; exit 1; }
echo "the live host was selected despite a vanished host.kv"
