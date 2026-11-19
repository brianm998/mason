#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s640 exit test: the exit gates use medians and condition waits, not single wall-clock samples.
set -euo pipefail
cd "$(dirname "$0")/../.."
bash -n scripts/exit/s11.sh scripts/exit/s23.sh scripts/exit/s426.sh
grep -q 'median' scripts/exit/s11.sh
# no bare sleep in s23.sh except the 30 s takeover waits, the fake speech command and
# the commented negative checks / the reload interval
bad=$(grep -n '^ *sleep ' scripts/exit/s23.sh | grep -Ev 'sleep 3[13]$|# ' || true)
[ -z "$bad" ] || { echo "bare sleep in s23.sh: $bad" >&2; exit 1; }
bash scripts/exit/s426.sh
echo "s640 OK"
