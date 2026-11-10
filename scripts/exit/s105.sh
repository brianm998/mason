#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Long waits (s105) exit test: a real masond, with [stall] long_wait = 1 (second) in notify.toml.
#  1. With MASON_CPU_FILE at 100 a compile waits; `mason alerts` raises long_wait for the compile
#     resource and names the CPU gate; dropping the reading lets it run and the alert clears.
#  2. A limit set to 0 and a request for it: `mason alerts` raises never_fits at once, with the fix.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s105.XXXXXX); daemon=""; cc=""; ww=""
cleanup() {
  [ -n "$cc" ] && kill "$cc" 2>/dev/null || true
  [ -n "$ww" ] && kill "$ww" 2>/dev/null || true
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { set +e; echo "FAILED: $*" >&2; m alerts >&2 2>&1; m resources >&2 2>&1; tail -20 "$root/masond.log" >&2; exit 1; }
# Wait until `mason alerts` matches the pattern (or, with "absent", stops matching).
alerts_match() {  # PATTERN present|absent
  local end=$((SECONDS + 30))
  while [ $SECONDS -lt $end ]; do
    if m alerts 2>/dev/null | grep -q "$1"; then [ "$2" = present ] && return 0
    else [ "$2" = absent ] && return 0; fi
    sleep 0.3
  done
  return 1
}

mkdir -p "$root/c" "$root/d"
printf '[stall]\nlong_wait = 1\n' > "$root/c/notify.toml"
echo 100 > "$root/cpu"
MASON_CPU_FILE="$root/cpu" "$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== 1. a compile held back by the CPU gate raises long_wait, and it clears when the CPU falls"
m run --needs compile -- touch "$root/compiled" 2>"$root/cc.err" &
cc=$!
alerts_match 'long_wait .*CPU 100% > 50%' present || fail "no long_wait alert naming the CPU gate"
[ ! -e "$root/compiled" ] || fail "the compile ran at CPU 100%"
m alerts | grep 'long_wait' | sed 's/^/   /'
echo 0 > "$root/cpu"
end=$((SECONDS + 30))
until [ -e "$root/compiled" ]; do [ $SECONDS -lt $end ] || fail "the compile did not run after CPU fell"; sleep 0.2; done
wait "$cc"; cc=""
alerts_match 'long_wait' absent || fail "the long_wait alert did not clear"
echo "   CPU fell: the compile ran and the alert cleared"

echo "== 2. a limit of 0 alerts at once"
m resources limit widget 0 >/dev/null
m run --needs widget -- true 2>"$root/ww.err" &
ww=$!
alerts_match 'never_fits .*mason resources limit widget 1' present || fail "no never_fits alert"
m alerts | grep 'never_fits' | sed 's/^/   /'
m resources limit widget 1 >/dev/null
wait "$ww"; ww=""
alerts_match 'never_fits' absent || fail "the never_fits alert did not clear"
echo "   raising the limit let it run and cleared the alert"

trap - EXIT; cleanup
echo "s105 OK"
