#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1035 exit test: the three scripts red on ubuntu shard 5/6 (s22, s244, s728) pass, are named in
# weights.txt, and none of them asserts on elapsed time.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
for n in 22 244 728; do
    grep -q "^s$n " scripts/exit/weights.txt || { echo "weights.txt has no entry for s$n" >&2; exit 1; }
    # wait loops bounded by $SECONDS are timeouts, not assertions; flag only a measured duration compared to a limit
    if grep -nE '(elapsed|took|duration|dur)[a-z_]* *-(lt|le|gt|ge)' "scripts/exit/s$n.sh" | grep -q .; then
        echo "s$n.sh has a timing assertion" >&2; exit 1
    fi
done
# Each sub-script's output is kept in a log and shown only when it fails: run-exit-scripts.sh takes
# the first line matching FAIL or "error" as the cause, and s22 prints "error" in a section header.
log=$(mktemp); trap 'rm -f "$log"' EXIT
t=""; command -v timeout >/dev/null && t="timeout -k 10 1200"
for n in 22 244 728; do
    echo "== scripts/exit/s$n.sh"
    rc=0; $t bash "scripts/exit/s$n.sh" >"$log" 2>&1 || rc=$?
    if [ "$rc" -ne 0 ]; then
        {
            echo "FAILED: s1035: scripts/exit/s$n.sh exit $rc"
            # a timed-out wait prints the daemon log after its message, which the tail would cut off
            grep -aE 'panicked at|FAILED|timed out' "$log" | head -n 5 | cut -c1-200
            echo "== s$n exit $rc, last 12 lines:"
            tail -n 12 "$log" | cut -c1-200
        } >&2
        exit 1
    fi
done
echo "s1035 OK"
