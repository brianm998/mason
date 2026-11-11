#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s116 exit test: a real masond (--debug) with the disk below the floor and store writes refused
# through /debug/store-writes. `mason alerts` still shows the disk alert, says the store cannot
# be written, and after writes are restored the alert is in the table once, with the same time.
set -euo pipefail
cd "$(dirname "$0")/../.."
root=$(mktemp -d /tmp/mason-s116.XXXXXX); pids=()
cleanup() { for p in "${pids[@]:-}"; do [ -n "$p" ] && kill -9 "$p" 2>/dev/null || true; done; rm -rf "$root"; }
trap cleanup EXIT
cargo build -q -p mason-daemon --bins -p mason-step-host --bins -p mason-mcp -p mason-pipeline --bins 2>/dev/null || cargo build -q --workspace --bins
bin=$PWD/target/debug
d=$root/d; c=$root/c; mkdir -p "$c" "$d"
echo 3 > "$root/disk"
MASON_DISK_FILE="$root/disk" MASON_DISK_EVERY_MS=100 "$bin/masond" --port 0 --debug --data-dir "$d" --config-dir "$c" \
  --host-bin "$bin/step-host" --mcp-bin "$bin/mason-mcp" --heartbeat-ms 100 --sweep-ms 3600000 2>"$root/masond.log" &
pids+=($!)
m=("$bin/mason" --data-dir "$d" --config-dir "$c")
for _ in $(seq 1 100); do
  kill -0 "${pids[0]}" 2>/dev/null || { cat "$root/masond.log" >&2; echo "masond exited" >&2; exit 1; }
  "${m[@]}" status >/dev/null 2>&1 && break
  sleep 0.2
done
"${m[@]}" status >/dev/null 2>&1 || { echo "masond never answered" >&2; exit 1; }
port=$(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1]))["port"])' "$d/daemon.json")
refuse() { curl -s -X POST "http://127.0.0.1:$port/debug/store-writes?refuse=$1" >/dev/null; }
alerts_json() { curl -s "http://127.0.0.1:$port/alerts"; }

echo "== 1. writes refused: the disk alert is still shown, with a note"
refuse 1
out=""
for _ in $(seq 1 100); do
  out=$("${m[@]}" alerts)
  echo "$out" | grep -q 'Disk nearly full' && break
  sleep 0.2
done
echo "$out" | grep -q 'Disk nearly full' || { echo "$out" >&2; echo "no disk alert" >&2; exit 1; }
echo "$out" | grep -q 'cannot write to its store' || { echo "$out" >&2; echo "no note" >&2; exit 1; }
since1=$(alerts_json | python3 -I -c 'import json,sys;a=[x for x in json.load(sys.stdin)["alerts"] if x["key"]=="disk"];assert len(a)==1;print(a[0]["since"])')

echo "== 2. writes restored: one alert, same time, no note"
refuse 0
for _ in $(seq 1 100); do
  out=$("${m[@]}" alerts)
  echo "$out" | grep -q 'cannot write' || break
  sleep 0.2
done
echo "$out" | grep -q 'cannot write' && { echo "$out" >&2; echo "note stayed" >&2; exit 1; }
since2=$(alerts_json | python3 -I -c 'import json,sys;a=[x for x in json.load(sys.stdin)["alerts"] if x["key"]=="disk"];assert len(a)==1,a;print(a[0]["since"])')
[ "$since1" = "$since2" ] || { echo "alert time changed: $since1 -> $since2" >&2; exit 1; }
echo "$("${m[@]}" alerts)" | grep -c 'Disk nearly full' | grep -qx 1 || { echo "alert duplicated" >&2; exit 1; }
echo ok
