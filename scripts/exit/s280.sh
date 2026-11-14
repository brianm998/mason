#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s280 exit test: `progress` and `doctor` are trait methods of GitHub, not an orphan impl block,
# and the e2e last-round count is the designed 6 (s282).
set -euo pipefail
cd "$(dirname "$0")/../.."
f=crates/adapters/src/github.rs
start=$(grep -n '^impl BuildSystemAdapter for GitHub' "$f" | cut -d: -f1)
end=$(awk -v s="$start" 'NR>s && /^}/ {print NR; exit}' "$f")
for m in progress doctor; do
  l=$(grep -n "^    fn $m(" "$f" | cut -d: -f1)
  [ "$l" -gt "$start" ] && [ "$l" -lt "$end" ]
done
grep -q 'trim(), "6"' crates/daemon/tests/e2e.rs
echo "s280 OK"
