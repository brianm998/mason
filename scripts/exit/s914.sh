#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s914: the six scripts that were red on the Linux shard 3 of the full run (s09 s40 s422 s546
# s669 s777) each pass, and a failure shows its whole output, not the tail.
set -uo pipefail
cd "$(git rev-parse --show-toplevel)"
bad=""
for n in s09 s40 s422 s546 s669 s777; do
  log=$(mktemp)
  if bash "scripts/exit/$n.sh" >"$log" 2>&1; then
    echo "s914: $n ok"
  else
    echo "s914: $n FAILED (exit $?), full output:" >&2
    cat "$log" >&2
    bad="$bad $n"
  fi
  rm -f "$log"
done
[ -z "$bad" ] || { echo "s914: failed:$bad" >&2; exit 1; }
echo "s914 OK"
