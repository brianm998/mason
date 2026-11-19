#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
set -uo pipefail
cd "$(dirname "$0")/../.."
out=$(cargo test -q -p mason-daemon --lib -- affected:: 2>&1); rc=$?
echo "$out" | tail -n 5
if [ $rc -eq 0 ] && echo "$out" | grep -q "test result" && ! echo "$out" | grep -q FAILED; then
  echo "PASS s616"; exit 0
fi
echo "FAIL s616"; exit 1
