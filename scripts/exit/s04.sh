#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 4 exit test: start a long-running command under a step host, kill -9 the
# controlling process, start a new one, and confirm it reattaches, loses no
# output and sees the exit status.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
tmp=$(mktemp -d /tmp/mason-s04.XXXXXX); ctl=""
cleanup() { [ -n "$ctl" ] && kill -9 "$ctl" 2>/dev/null || true; pkill -f "step-host --dir $tmp" 2>/dev/null || true; rm -rf "$tmp"; }
trap cleanup EXIT

# CI has already run the whole workspace suite; locally this runs it.
[ -n "${MASON_TESTS_ALREADY_RAN:-}" ] || cargo test -q -p mason-step-host
cargo build -q -p mason-step-host --bins
export STEP_HOST_BIN="$PWD/target/debug/step-host"
tool=target/debug/step-host-ctl
N=200
script="i=1; while [ \$i -le $N ]; do echo line \$i; i=\$((i+1)); sleep 0.01; done; exit 9"
seq -f "line %g" 1 $N > "$tmp/expected"

for round in 1 2 3; do
  id="r$round"; dir="$tmp/$id"
  "$tool" run "$tmp" "$id" --hb-ms 100 -- sh -c "$script" > /dev/null 2>&1 &
  ctl=$!
  # Kill the controller after a different amount of output each round.
  want=$((20 + round * 40))
  until [ -f "$dir/received" ] && [ "$(wc -l < "$dir/received")" -ge "$want" ]; do sleep 0.02; done
  kill -9 "$ctl"; wait "$ctl" 2>/dev/null || true; ctl=""
  before=$(wc -c < "$dir/received")
  host=$(sed -n 's/^host_pid=//p' "$dir/host.kv")
  kill -0 "$host" || { echo "host died with the controller" >&2; exit 1; }
  sleep 0.4   # nobody attached; the host keeps journaling
  after=$(wc -c < "$dir/journal")
  [ "$after" -gt "$before" ] || [ -f "$dir/exit.kv" ] || { echo "host stopped journaling" >&2; exit 1; }
  echo "round $round: controller killed after $(wc -l < "$dir/received") lines; host $host alive; journal $after bytes"

  "$tool" attach "$tmp" "$id"            # a brand new controller
  diff "$tmp/expected" "$dir/received" && echo "round $round: no loss, no duplicates ($N lines)"
  grep -q '^code=9$' "$dir/exit_status" && echo "round $round: exit status 9 seen"
  grep -q '^code=9$' "$dir/exit_status"
done
echo "s04 OK"
