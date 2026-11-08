#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# A device suite for the fake provider. Usage: suite-tap.sh LOG GATE_DIR [FLAG]
#   - appends "suite <MASON_DEVICE>" to LOG (the fake provider's call log),
#   - holds while GATE_DIR/hold exists, so a test can watch who holds what,
#   - fails the way a test would if FLAG exists and the change added android/red.txt (a product failure that
#     the merge base does not share, so it is not excused as a baseline failure),
#   - otherwise prints three passing TAP tests.
log=$1
gate=$2
flag=${3:-}
echo "suite ${MASON_DEVICE:-none}" >> "$log"
while [ -f "$gate/hold" ]; do sleep 0.2; done
if [ -n "$flag" ] && [ -f "$flag" ] && [ -f android/red.txt ]; then
  echo "1..3"; echo "ok 1 - one"; echo "not ok 2 - two"; echo "ok 3 - three"
  exit 1
fi
echo "1..3"; echo "ok 1 - one"; echo "ok 2 - two"; echo "ok 3 - three"
