#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Fail unless every mason-daemon test is in exactly one of the N shards (default 3).
# Usage: check-daemon-shards.sh [N] [LISTFILE]
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
n=${1:-3}
list=${2:-}
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
if [ -z "$list" ]; then
  list=$tmp/list
  cargo test -p mason-daemon -- --list --format terse 2>/dev/null >"$list"
fi
grep ': test$' "$list" | sed 's/: test$//' | sort -u >"$tmp/all"
: >"$tmp/shards"
k=1
while [ "$k" -le "$n" ]; do
  bash "$here/daemon-shard.sh" "$k" "$n" "$list" | tee "$tmp/s$k" >>"$tmp/shards"
  k=$((k + 1))
done
total=$(wc -l <"$tmp/all" | tr -d ' ')
[ "$total" -gt 0 ] || { echo "no tests listed" >&2; exit 1; }
dups=$(sort "$tmp/shards" | uniq -d)
[ -z "$dups" ] || { echo "in more than one shard:" >&2; echo "$dups" >&2; exit 1; }
missing=$(sort "$tmp/shards" | comm -23 "$tmp/all" -)
[ -z "$missing" ] || { echo "in no shard:" >&2; echo "$missing" >&2; exit 1; }
echo "ok: $total tests in $n shards"
