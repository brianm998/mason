#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s103 exit test: the watchdog outside masond.
#   0. the shell tests (scripts/tests/watchdog.sh) pass;
#   1. a real daemon answers: the watchdog stays quiet;
#   2. after kill -9, two runs report down through a stub notifier;
#   3. a listener that accepts and never answers also reports down;
#   4. a restart reports back up exactly once.
set -euo pipefail
cd "$(dirname "$0")/../.."
root=$(mktemp -d /tmp/mason-s103.XXXXXX); pids=()
cleanup() { for p in "${pids[@]:-}"; do [ -n "$p" ] && kill -9 "$p" 2>/dev/null || true; done; rm -rf "$root"; }
trap cleanup EXIT
bash scripts/tests/watchdog.sh >/dev/null
cargo build -q -p mason-daemon --bins -p mason-step-host --bins -p mason-mcp 2>/dev/null || cargo build -q --workspace --bins
bin=$PWD/target/debug
d=$root/d; c=$root/c; mkdir -p "$c" "$d"
args=(--port 0 --data-dir "$d" --config-dir "$c" --host-bin "$bin/step-host" --mcp-bin "$bin/mason-mcp" --heartbeat-ms 100 --sweep-ms 3600000)
cat >"$root/say" <<SH
#!/bin/bash
echo "\$*" >> "$root/said"
SH
chmod +x "$root/say"; : >"$root/said"
export MASON_WATCHDOG_STATE=$root/state MASON_WATCHDOG_DAEMON_JSON=$d/daemon.json
export MASON_WATCHDOG_NOTIFY_TOML=$root/none.toml MASON_WATCHDOG_SAY=$root/say MASON_WATCHDOG_TIMEOUT=2
minute=0
# Simulated minutes, except while a real daemon is up: its uptime follows the real clock.
run() { minute=$((minute + 1)); MASON_WATCHDOG_NOW=$((2000000 + minute * 60)) bash scripts/mason-watchdog.sh; }
runreal() { env -u MASON_WATCHDOG_NOW bash scripts/mason-watchdog.sh; }
count() { awk -v p="$1" 'index($0, p) { n++ } END { print n + 0 }' "$root/said"; }
start() {
  rm -f "$d/daemon.json"
  "$bin/masond" "${args[@]}" 2>"$root/masond.log" & dpid=$!; pids+=($dpid)
  for _ in $(seq 1 100); do
    kill -0 "$dpid" 2>/dev/null || { cat "$root/masond.log" >&2; echo "masond exited" >&2; exit 1; }
    [ -s "$d/daemon.json" ] && "$bin/mason" --data-dir "$d" --config-dir "$c" status >/dev/null 2>&1 && return
    sleep 0.2
  done
  echo "masond never answered" >&2; exit 1
}

echo "== 1. a real daemon answers"
start
runreal; runreal; runreal
[ ! -s "$root/said" ] || { cat "$root/said" >&2; echo "watchdog spoke while the daemon was up" >&2; exit 1; }

echo "== 2. kill -9: two runs say down"
kill -9 "$dpid"; wait "$dpid" 2>/dev/null || true
run; [ ! -s "$root/said" ] || { echo "spoke after one failure" >&2; exit 1; }
run; [ "$(count 'is down')" = 1 ] || { cat "$root/said" >&2; echo "no single 'down'" >&2; exit 1; }

echo "== 3. a daemon that accepts and never answers"
python3 -I -c '
import socket,sys,time
s=socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1",0)); s.listen(5)
open(sys.argv[1],"w").write(str(s.getsockname()[1]))
conns=[]
while True: conns.append(s.accept())' "$root/hungport" &
pids+=($!)
until [ -s "$root/hungport" ]; do sleep 0.1; done
: >"$root/said"; rm -f "$MASON_WATCHDOG_STATE"
printf '{"pid":1,"port":%s,"started_ms":1}' "$(cat "$root/hungport")" >"$d/daemon.json"
run; [ ! -s "$root/said" ] || { echo "spoke after one failure" >&2; exit 1; }
run; [ "$(count 'is down')" = 1 ] || { cat "$root/said" >&2; echo "hung daemon not reported down" >&2; exit 1; }

echo "== 4. a restart: back, once"
start
runreal; runreal; runreal
[ "$(count 'is back')" = 1 ] || { cat "$root/said" >&2; echo "expected one 'back'" >&2; exit 1; }
[ "$(count 'crash-looping')" = 0 ] || { echo "a single restart was called a crash loop" >&2; exit 1; }
echo "s103 OK"
