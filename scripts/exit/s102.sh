#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s102 exit test: a disk reading below the floor reaches the owner. A real masond with
# MASON_DISK_FILE holding 3 (GB, below the 20 GB floor):
#   1. `mason notify status` lists the disk route (monitor, sound, speech, push after 10 min);
#   2. a client claiming over HTTP is handed one announcement of kind disk, with sound and speech.
set -euo pipefail
cd "$(dirname "$0")/../.."
root=$(mktemp -d /tmp/mason-s102.XXXXXX); pids=()
cleanup() { for p in "${pids[@]:-}"; do [ -n "$p" ] && kill -9 "$p" 2>/dev/null || true; done; rm -rf "$root"; }
trap cleanup EXIT
cargo build -q -p mason-daemon --bins -p mason-step-host --bins -p mason-mcp -p mason-pipeline --bins 2>/dev/null || cargo build -q --workspace --bins
bin=$PWD/target/debug
d=$root/d; c=$root/c; mkdir -p "$c" "$d"
echo 3 > "$root/disk"
MASON_DISK_FILE="$root/disk" MASON_DISK_EVERY_MS=100 "$bin/masond" --port 0 --data-dir "$d" --config-dir "$c" \
  --host-bin "$bin/step-host" --mcp-bin "$bin/mason-mcp" --heartbeat-ms 100 --sweep-ms 3600000 2>"$root/masond.log" &
pids+=($!)
m=("$bin/mason" --data-dir "$d" --config-dir "$c")
for _ in $(seq 1 100); do
  kill -0 "${pids[0]}" 2>/dev/null || { cat "$root/masond.log" >&2; echo "masond exited" >&2; exit 1; }
  "${m[@]}" status >/dev/null 2>&1 && break
  sleep 0.2
done
"${m[@]}" status >/dev/null 2>&1 || { echo "masond never answered" >&2; exit 1; }

echo "== 1. mason notify status shows the disk route"
out=$("${m[@]}" notify status)
echo "$out" | grep -E '^disk +monitor, sound, speech, push after 10 min$' >/dev/null \
  || { echo "$out" >&2; echo "no disk route" >&2; exit 1; }

echo "== 2. a client claiming over HTTP is handed the disk announcement"
port=$(python3 -I -c 'import json,sys;print(json.load(open(sys.argv[1]))["port"])' "$d/daemon.json")
found=""
for _ in $(seq 1 100); do
  r=$(curl -s -X POST "http://127.0.0.1:$port/announce/claim" -H 'content-type: application/json' \
      -d '{"client":"s102","muted":false}')
  if python3 -I -c '
import json,sys
a=[x for x in json.loads(sys.argv[1])["announcements"] if x["kind"]=="disk"]
assert len(a)==1 and a[0]["sound"] is True and a[0]["speech"] is True, a
' "$r" 2>/dev/null; then found=1; break; fi
  sleep 0.3
done
[ -n "$found" ] || { echo "no disk announcement; last claim: $r" >&2; cat "$root/masond.log" >&2; exit 1; }
echo "s102 OK"
