#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s675 exit test: a daemon test killed with SIGKILL while its blocking world runs leaves no
# step host, no test masond and no /tmp/ms* root once another test of the binary has started.
set -uo pipefail
cd "$(dirname "$0")/../.."
before=$(mktemp /tmp/s675-before.XXXXXX)
out=$(mktemp /tmp/s675-out.XXXXXX)
trap 'rm -f "$before" "$out"' EXIT
ls -d /tmp/ms* 2>/dev/null | sort >"$before"

# Build once, so the killed run starts at once.
cargo test -q -p mason-daemon --test it --no-run >/dev/null 2>&1 || { echo "s675: build failed"; exit 1; }

cargo test -q -p mason-daemon --test it a_takeover_waits_for_running_landing_batches >"$out" 2>&1 &
cargo_pid=$!
# Partway: the test binary is cargo's child; its world root is the one whose owner.lock holds
# its pid (other runs on this machine make roots too), and it has a host under it.
root=""
tpid=""
for _ in $(seq 1 1200); do
  tpid=$(pgrep -P "$cargo_pid" | head -1)
  if [ -n "$tpid" ]; then
    for f in /tmp/ms*/owner.lock; do
      [ -e "$f" ] || continue
      [ "$(cat "$f" 2>/dev/null)" = "$tpid" ] || continue
      r=$(dirname "$f")
      if /bin/ps -axo command= | grep -F -- "/tmp/$(basename "$r")/" | grep -q -e '--dir' -e '--data-dir'; then root=$r; break 2; fi
    done
  fi
  sleep 0.5
done
[ -n "$root" ] || { echo "s675: no blocking world appeared"; kill "$cargo_pid" 2>/dev/null; exit 1; }
echo "killing the run whose root is $root"
# SIGKILL the test binary (and cargo), not its hosts.
kill -KILL "$tpid" 2>/dev/null
kill -KILL "$cargo_pid" 2>/dev/null
wait "$cargo_pid" 2>/dev/null

# Another test of the same binary: its first world sweeps.
cargo test -q -p mason-daemon --test it a_failed_takeover_lets_landing_resume >"$out" 2>&1 || { echo "s675: the second test failed"; tail -20 "$out"; exit 1; }

left=$(/bin/ps -axo pid=,command= | grep -F -- "$root/" | grep -v grep || true)
if [ -n "$left" ]; then echo "s675: processes remain under $root:"; echo "$left"; exit 1; fi
if [ -e "$root" ]; then echo "s675: $root remains"; exit 1; fi
echo "s675: ok, $root and its hosts are gone"
