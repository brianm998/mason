#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s238 exit test: the compile gates change at runtime, are kept across a restart, and reset.
# A real masond whose CPU reading is a file (MASON_CPU_FILE) says 60%, with one compile held.
#  1. A second compile waits on the CPU gate (default 50).
#  2. `mason resources gate compile_cpu_max 70` grants it with no restart.
#  3. After a restart `mason resources --json` still says 70, source "runtime".
#  4. `--reset` brings back 50, source "default".
#  5. `conformance features clients/features.toml` reports runtime-gates done in both clients.
# Not covered here: the monitors' steppers (their core tests and the Swift view test do that).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s238.XXXXXX); daemon=""; cc=""; first=""
cleanup() {
  [ -n "$first" ] && { touch "$root/release"; kill "$first" 2>/dev/null; } || true
  [ -n "$cc" ] && kill "$cc" 2>/dev/null || true
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { set +e; echo "FAILED: $*" >&2; m resources >&2 2>&1; tail -20 "$root/masond.log" >&2; exit 1; }
start() {
  MASON_CPU_FILE="$root/cpu" "$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" \
    --heartbeat-ms 100 --sweep-ms 3600000 2>>"$root/masond.log" &
  daemon=$!
  until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
}
gate_json() { m resources --json | python3 -I -c "
import json,sys
g=[g for g in json.load(sys.stdin)['gates'] if g['name']=='compile_cpu_max'][0]
print(g['value'], g['source'])"; }

mkdir -p "$root/c" "$root/d"
echo 60 > "$root/cpu"
start
m resources limit compile 2 >/dev/null

echo "== 1. one compile held, CPU reads 60% and the gate is 50: the second waits"
m run --needs compile -- sh -c "until [ -e '$root/release' ]; do sleep 0.1; done" 2>"$root/first.err" &
first=$!
end=$((SECONDS + 20))
until m resources | grep -q 'compile .* 1/2 in use'; do [ $SECONDS -lt $end ] || fail "the first compile did not start"; sleep 0.2; done
m run --needs compile -- touch "$root/compiled" 2>"$root/cc.err" &
cc=$!
sleep 2
[ ! -e "$root/compiled" ] || fail "the second compile ran at 60% with the gate at 50"
[ "$(gate_json)" = "50 default" ] || fail "expected 50 default, got $(gate_json)"
echo "   held"

echo "== 2. gate compile_cpu_max 70: it is granted without a restart"
m resources gate compile_cpu_max 70 >/dev/null || fail "setting the gate"
end=$((SECONDS + 30))
until [ -e "$root/compiled" ]; do [ $SECONDS -lt $end ] || fail "the waiting compile was not granted"; sleep 0.2; done
wait "$cc"; cc=""
touch "$root/release"; wait "$first" || true; first=""
[ "$(gate_json)" = "70 runtime" ] || fail "expected 70 runtime, got $(gate_json)"
m resources gate compile_cpu_max 101 >/dev/null 2>&1 && fail "101 was accepted"
[ "$(gate_json)" = "70 runtime" ] || fail "a refused value changed the gate"
echo "   granted"

echo "== 3. restart: 70 from runtime"
kill -9 "$daemon"; wait "$daemon" 2>/dev/null || true; daemon=""
start
[ "$(gate_json)" = "70 runtime" ] || fail "after a restart: $(gate_json)"
echo "   kept"

echo "== 4. --reset: back to 50"
m resources gate compile_cpu_max --reset >/dev/null || fail "reset"
[ "$(gate_json)" = "50 default" ] || fail "after reset: $(gate_json)"
echo "   reset"

echo "== 5. the feature is done in both clients"
cargo run -q -p mason-conformance -- features clients/features.toml | tee "$root/features.out" \
  || fail "conformance features failed"
python3 -I - clients/features.toml <<'PY' || fail "runtime-gates is not done in both clients"
import sys, tomllib
f = tomllib.load(open(sys.argv[1], "rb"))["feature"]["runtime-gates"]
sys.exit(0 if f["swift"] == "done" and f["compose"] == "done" else 1)
PY

trap - EXIT; cleanup
echo "s238 OK"
