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

root=""; tpid=""
# The window is a few seconds on a fast machine: a run that ends before it was seen is run again.
for attempt in 1 2 3; do
  cargo test -q -p mason-daemon --test it a_takeover_waits_for_running_landing_batches >"$out" 2>&1 &
  cargo_pid=$!
  # Partway: the test binary is the `it` binary (cargo may be wrapped, so not necessarily its
  # child) that holds a new root's owner.lock, with a host under that root. Other runs on this
  # machine make roots too; their lock pids name other binaries or are not `it` runs of ours.
  # Wait on the run itself, not a clock: on a loaded machine the world first waits up to ten
  # minutes for a shared world slot. A run that ends without one is reported below.
  while kill -0 "$cargo_pid" 2>/dev/null; do
    for r in $(ls -d /tmp/ms* 2>/dev/null | sort | comm -13 "$before" -); do
      p=$(cat "$r/owner.lock" 2>/dev/null)
      [ -n "$p" ] || continue
      /bin/ps -o command= -p "$p" 2>/dev/null | grep -q '/deps/it-' || continue
      # Ours: a descendant of the cargo job we started.
      q=$p
      ours=0
      for _ in 1 2 3 4 5 6; do
        q=$(/bin/ps -o ppid= -p "$q" 2>/dev/null | tr -d ' ')
        [ -n "$q" ] || break
        [ "$q" = "$cargo_pid" ] && { ours=1; break; }
      done
      [ "$ours" = 1 ] || continue
      if /bin/ps -axo command= | grep -F -- "/tmp/$(basename "$r")/" | grep -q -e '--dir' -e '--data-dir'; then root=$r; tpid=$p; break 2; fi
    done
    sleep 0.1
  done
  [ -n "$root" ] && break
  echo "s675: attempt $attempt ended before a blocking world appeared"; tail -5 "$out"
done
[ -n "$root" ] || { echo "s675: the run ended before a blocking world appeared"; tail -20 "$out"; exit 1; }
echo "killing the run whose root is $root"
# SIGKILL the test binary (and cargo), not its hosts.
kill -KILL "$tpid" 2>/dev/null
kill -KILL "$cargo_pid" 2>/dev/null
wait "$cargo_pid" 2>/dev/null

# Another test of the same binary: its first world sweeps.
cargo test -q -p mason-daemon --test it a_failed_takeover_lets_landing_resume >"$out" 2>&1 || { echo "s675: the second test (a_failed_takeover_lets_landing_resume) failed"; tail -20 "$out"; exit 1; }

left=$(/bin/ps -axo pid=,command= | grep -F -- "$root/" | grep -v grep || true)
if [ -n "$left" ]; then echo "s675: processes remain under $root:"; echo "$left"; tail -20 "$out"; exit 1; fi
# Dead roots are removed on a background thread, so wait a bounded time for it.
for _ in $(seq 1 120); do [ -e "$root" ] || break; sleep 0.5; done
if [ -e "$root" ]; then echo "s675: $root remains after 60 s"; ls -la "$root" | head -20; tail -20 "$out"; exit 1; fi
echo "s675: ok, $root and its hosts are gone"
