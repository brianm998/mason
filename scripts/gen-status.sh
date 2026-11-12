#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Builds the status block of README.md from the stage notes (docs/stages/sNN.md), one bullet per
# note in number order: the note's `Summary:` line, or its heading when it has none. MASON runs
# this when changes land; tasks never edit the block, so two tasks can land in either order
# without touching the same line.
#   gen-status.sh           rewrite the block in README.md
#   gen-status.sh --print   print the block that README.md should hold
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
begin='<!-- status:begin -->'
end='<!-- status:end -->'

block() {
  echo "$begin"
  echo "What each stage note says it built, oldest first (generated from \`docs/stages\` by \`scripts/gen-status.sh\` when a change lands; do not edit between the markers):"
  echo
  for f in docs/stages/s[0-9][0-9].md docs/stages/s[0-9][0-9][0-9].md; do
    [ -f "$f" ] || continue
    n=$(basename "$f" .md)
    sum=$(sed -n 's/^Summary:[[:space:]]*//p' "$f" | head -1)
    [ -n "$sum" ] || sum=$(sed -n 's/^#[[:space:]]*//p' "$f" | head -1)
    echo "- **$n**: $sum"
  done
  echo "$end"
}

if [ "${1:-}" = "--print" ]; then block; exit 0; fi
grep -q "^$begin\$" README.md && grep -q "^$end\$" README.md \
  || { echo "README.md has no status block markers ($begin ... $end)" >&2; exit 1; }
tmp=$(mktemp)
block > "$tmp.block"
awk -v blockfile="$tmp.block" -v b="$begin" -v e="$end" '
  $0 == b { while ((getline line < blockfile) > 0) print line; skip = 1; next }
  $0 == e { skip = 0; next }
  !skip { print }
' README.md > "$tmp"
cat "$tmp" > README.md
rm -f "$tmp" "$tmp.block"
