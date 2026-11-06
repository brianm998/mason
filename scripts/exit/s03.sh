#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 3 exit test: append events from a separate process, kill -9 it at
# varying points, reopen the store, and confirm nothing acknowledged is lost and
# nothing is half-applied (log contiguous, projections equal to a replay).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
tmp=$(mktemp -d); pid=""
trap '[ -n "$pid" ] && kill -9 "$pid" 2>/dev/null || true; rm -rf "$tmp"' EXIT

cargo test -p mason-store
cargo build -q -p mason-store --example crash_tool
tool=target/debug/examples/crash_tool
db="$tmp/crash.db"

for round in 1 2 3 4 5 6; do
  out="$tmp/out.$round"; : > "$out"
  "$tool" write "$db" > "$out" 2>"$tmp/err.$round" &
  pid=$!
  want=$((40 + round * 37))
  until [ "$(grep -cE '^ACK [0-9]+$' "$out" || true)" -ge "$want" ]; do
    kill -0 "$pid" 2>/dev/null || { echo "writer died early:" >&2; cat "$tmp/err.$round" >&2; exit 1; }
    sleep 0.05
  done
  kill -9 "$pid"; wait "$pid" 2>/dev/null || true; pid=""
  last_ack=$(grep -E '^ACK [0-9]+$' "$out" | tail -1 | cut -d' ' -f2)
  echo "round $round: killed writer after ack $last_ack"
  "$tool" check "$db" "$last_ack"
done
echo "s03 OK"
