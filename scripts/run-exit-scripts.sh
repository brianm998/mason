#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Runs every scripts/exit/sNN.sh that needs no real service, person or device.
# Scripts needing those are named sNN-real.sh or sNN-manual.sh and are skipped.
set -uo pipefail
cd "$(git rev-parse --show-toplevel)"
# A script's daemon must not act on what else this machine is doing, and a real
# CPU reading would also replace the compile counts the scripts set up. A script
# that drives the gate itself sets its own file.
cpu=$(mktemp); echo 0 > "$cpu"; export MASON_CPU_FILE="$cpu"
trap 'rm -f "$cpu"' EXIT
fail=0
for s in scripts/exit/s[0-9][0-9].sh; do
  [ -e "$s" ] || continue
  echo "== $s"
  bash "$s" || { echo "FAILED: $s" >&2; fail=1; }
done
exit $fail
