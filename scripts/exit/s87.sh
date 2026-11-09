#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s87 exit test: a refused start is not a crash, and a stale daemon.json does not block.
#   1. a port held by another process, masond --managed started three times: each
#      exits 1, boots.log says `refused`, no rollback, no alert;
#   2. a daemon.json holding a live unrelated pid: masond starts.
set -euo pipefail
cd "$(dirname "$0")/../.."
root=$(mktemp -d /tmp/mason-s87.XXXXXX); pids=()
cleanup() { for p in "${pids[@]:-}"; do [ -n "$p" ] && kill -9 "$p" 2>/dev/null || true; done; rm -rf "$root"; }
trap cleanup EXIT
cargo build -q -p mason-daemon --bins -p mason-step-host --bins -p mason-mcp 2>/dev/null || cargo build -q --workspace --bins
bin=$PWD/target/debug
d=$root/d; c=$root/c; mkdir -p "$c" "$d"
sha=0123456789abcdef0123456789abcdef01234567
rt=$d/runtimes/$sha; mkdir -p "$rt"
for b in masond step-host mason-mcp; do cp "$bin/$b" "$rt/$b"; done
touch "$rt/READY"; echo "$sha" > "$d/runtimes/current"
args=(--managed --port 0 --data-dir "$d" --config-dir "$c" --host-bin "$bin/step-host" --mcp-bin "$bin/mason-mcp" --heartbeat-ms 100 --sweep-ms 3600000)

echo "== 1. a held port, three managed starts"
python3 -I -c '
import socket,sys,time
s=socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1",0)); s.listen(1)
open(sys.argv[1],"w").write(str(s.getsockname()[1])); time.sleep(600)' "$root/port" &
pids+=($!)
until [ -s "$root/port" ]; do sleep 0.1; done
port=$(cat "$root/port")
for i in 1 2 3; do
  set +e; "$bin/masond" "${args[@]}" --port "$port" 2>"$root/err$i"; code=$?; set -e
  [ "$code" = 1 ] || { echo "start $i exited $code" >&2; cat "$root/err$i" >&2; exit 1; }
  grep -q "is taken by" "$root/err$i" || { echo "no port message" >&2; cat "$root/err$i" >&2; exit 1; }
done
[ "$(grep -c ' refused$' "$d/runtimes/boots.log")" = 3 ] || { cat "$d/runtimes/boots.log" >&2; echo "boots.log lacks three refused" >&2; exit 1; }
# A fourth start would roll back if the refusals counted.
set +e; "$bin/masond" "${args[@]}" --port "$port" 2>/dev/null; set -e
[ ! -e "$d/runtimes/rollback" ] || { echo "a rollback was recorded" >&2; exit 1; }
[ ! -e "$d/runtimes/alert" ] || { echo "an alert was left" >&2; cat "$d/runtimes/alert" >&2; exit 1; }
[ ! -e "$d/runtimes/bad" ] || { echo "a build was marked bad" >&2; exit 1; }

echo "== 2. a stale daemon.json naming a live, unrelated pid"
sleep 600 & other=$!; pids+=($other)
printf '{"pid":%s,"port":1,"started_ms":1}' "$other" > "$d/daemon.json"
"$bin/masond" "${args[@]}" 2>"$root/masond.log" &
dpid=$!; pids+=($dpid)
for _ in $(seq 1 100); do
  kill -0 "$dpid" 2>/dev/null || { cat "$root/masond.log" >&2; echo "masond exited" >&2; exit 1; }
  "$bin/mason" --data-dir "$d" --config-dir "$c" status >/dev/null 2>&1 && break
  sleep 0.2
done
"$bin/mason" --data-dir "$d" --config-dir "$c" status >/dev/null 2>&1 || { echo "masond never answered" >&2; exit 1; }
echo "s87 OK"
