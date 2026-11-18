#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s599: every `--test NAME` in the smoke list must be a real cargo test target.
set -uo pipefail
cd "$(dirname "$0")/../.."
fail=0
while read -r line; do
  case "$line" in ''|'#'*) continue ;; esac
  set -- $line
  name=""
  while [ $# -gt 0 ]; do
    if [ "$1" = "--test" ]; then name="${2:-}"; fi
    shift
  done
  [ -z "$name" ] && continue
  found=0
  for f in crates/*/tests/"$name".rs crates/*/tests/"$name"/main.rs; do
    [ -f "$f" ] && found=1
  done
  if [ $found = 0 ]; then echo "FAIL s599: no test target '$name' for line: $line"; fail=1; fi
done < .mason/smoke-tests.txt
grep -q '^mod landing_floor;' crates/daemon/tests/it/main.rs || { echo "FAIL s599: landing_floor module missing"; fail=1; }
[ $fail = 0 ] && echo "PASS s599"
exit $fail
