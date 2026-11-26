#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s914: the six scripts that were red on the Linux shard 3 of the full run (s09 s40 s422 s546
# s669 s777) each pass, and a failure names its last lines together at the end, not scattered.
set -uo pipefail
cd "$(git rev-parse --show-toplevel)"
t=""; command -v timeout >/dev/null && t="timeout -k 10 1200"
bad=""; report=""
for n in s09 s40 s422 s546 s669 s777; do
  log=$(mktemp)
  if $t bash "scripts/exit/$n.sh" >"$log" 2>&1; then
    echo "s914: $n ok"
  else
    rc=$?
    bad="$bad $n (exit $rc)"
    report="$report
== $n exit $rc, last 12 lines:
$(tail -12 "$log" | cut -c1-200)"
  fi
  rm -f "$log"
done
# $bad carries each exit code; "FAILED" makes this line the cause run-exit-scripts.sh reports.
if [ -n "$bad" ]; then echo "FAILED: s914:$bad$report" >&2; exit 1; fi
echo "s914 OK"
