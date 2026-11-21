#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s775: the joint-failure test must form its batch by max_size, not a 2s window.
set -uo pipefail
cd "$(dirname "$0")/../.."
f=crates/daemon/tests/it/landing_floor.rs
body=$(awk '/^fn two_changes_that_merge_cleanly/{p=1} p&&/^}/{exit} p' "$f")
if echo "$body" | grep -q 'max_size = 2' && ! echo "$body" | grep -q 'window = \\"2s\\"'; then
  echo "PASS s775"
else
  echo "FAIL s775: joint test must use max_size = 2 and no 2s window"
  exit 1
fi
