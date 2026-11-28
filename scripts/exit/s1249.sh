#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1249 exit test: the cli init test is gated to Unix (decision 1239).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1249.md
grep -q '^#!\[cfg(unix)\]$' crates/cli/tests/init.rs
for f in $(grep -rl 'std::os::unix' crates/cli/tests); do
  grep -q '^#!\[cfg(unix)\]$' "$f" || { echo "unguarded std::os::unix in $f"; exit 1; }
done
echo "OK s1249"
