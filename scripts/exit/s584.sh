#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s584 exit test: every `-p` package in .mason/smoke-tests.txt exists in the workspace.
set -uo pipefail
cd "$(dirname "$0")/../.."
fail=0
pkgs=$(cargo metadata --no-deps --format-version 1 | python3 -I -c '
import json,sys
for p in json.load(sys.stdin)["packages"]: print(p["name"])') || { echo "FAIL s584: cargo metadata"; exit 1; }
while read -r name; do
  echo "$pkgs" | grep -qx "$name" || { echo "FAIL s584: smoke list names unknown package $name"; fail=1; }
done < <(grep -v '^[[:space:]]*#' .mason/smoke-tests.txt | sed -n 's/.*-p[[:space:]]\{1,\}\([^[:space:]]\{1,\}\).*/\1/p')
cargo test -q -p mason-pipeline --test init --no-run || fail=1
if [ "$fail" = 0 ]; then echo "PASS s584"; else echo "FAIL s584"; fi
exit $fail
