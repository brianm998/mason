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
for n in 22 244 728; do
    echo "== scripts/exit/s$n.sh"
    "scripts/exit/s$n.sh"
done
echo "s1035 OK"
