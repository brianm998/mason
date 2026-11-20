#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Print the names of the mason-daemon tests that belong to shard K of N (1-based), one per line.
# A test's shard is a checksum of its name, so every test is in exactly one shard and a new
# test is assigned without editing a list (task 720, decision 0714).
#   scripts/daemon-shard.sh K N [LISTFILE]   LISTFILE is `cargo test -- --list` output; when
#                                            absent it is produced here.
set -euo pipefail
k=${1:?shard}; n=${2:?count}
list=${3:-}
if [ -z "$list" ]; then
  list=$(mktemp); trap 'rm -f "$list"' EXIT
  cargo test -p mason-daemon -- --list --format terse 2>/dev/null >"$list"
fi
grep ': test$' "$list" | sed 's/: test$//' | sort -u | while IFS= read -r name; do
  sum=$(printf '%s' "$name" | cksum | cut -d' ' -f1)
  if [ $((sum % n + 1)) -eq "$k" ]; then printf '%s\n' "$name"; fi
done
