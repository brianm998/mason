#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s90 exit test: a full main worker pool does not blank a monitor or stop `mason status`.
#   A real masond --debug; one route holds a repository lock, and enough requests
#   wait on it to fill the blocking pool (4 threads + 64 queued). Then a new
#   WebSocket client must still get a full snapshot and `mason status` answers,
#   showing the pools' queue depth.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s90.XXXXXX); daemon=""; waiters=()
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  for p in ${waiters[@]+"${waiters[@]}"}; do
    # It may exit between the check and the kill; a failed kill is fine only if it is gone.
    if kill -0 "$p" 2>/dev/null; then kill "$p" 2>/dev/null || ! kill -0 "$p" 2>/dev/null; fi
  done
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
cargo build -q -p mason-daemon --example snapshot_probe
bin=target/debug
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }

git init -q -b main "$root/repo"
echo scratch > "$root/repo/README"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm init
mkdir -p "$root/c/projects" "$root/d"
cat > "$root/c/projects/scratch.toml" <<TOML
path = "$root/repo"
base = "main"
test = "true"
TOML

"$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --port 0 --debug \
  --host-bin "$bin/step-host" --mcp-bin "$bin/mason-mcp" 2>"$root/masond.log" &
daemon=$!
for _ in $(seq 1 100); do
  m status >/dev/null 2>&1 && break
  kill -0 "$daemon" 2>/dev/null || { cat "$root/masond.log" >&2; echo "masond exited" >&2; exit 1; }
  sleep 0.2
done
port=$(python3 -I -c 'import json,sys; print(json.load(open(sys.argv[1]))["port"])' "$root/d/daemon.json")
url=http://127.0.0.1:$port

echo "== a snapshot before the load"
"$bin/../debug/examples/snapshot_probe" "$port" | tee "$root/before"
grep -q 'views=[1-9]' "$root/before"

echo "== fill the blocking pool with work waiting on the repository lock"
curl -fsS -X POST "$url/debug/hold-repo-lock?ms=20000" >/dev/null
for _ in $(seq 1 90); do
  curl -s -m 25 -X POST "$url/debug/wait-repo" >/dev/null 2>&1 &
  waiters+=($!)
done
full=""
for _ in $(seq 1 50); do
  q=$(m status --json | python3 -I -c 'import json,sys; p=json.load(sys.stdin)["pools"]["blocking"]; print(p["queued"], p["queue_capacity"])')
  [ "$q" = "64 64" ] && { full=1; break; }
  sleep 0.2
done
[ -n "$full" ] || { echo "the blocking pool never filled ($q)" >&2; exit 1; }

echo "== while it is full"
"$bin/../debug/examples/snapshot_probe" "$port" | tee "$root/during"
grep -q 'views=[1-9]' "$root/during" || { echo "empty snapshot" >&2; exit 1; }
# Into a file first: `tee | grep -q` under pipefail fails when grep stops reading early.
m status > "$root/status"; cat "$root/status"
grep -q 'pool blocking: 64 queued of 64' "$root/status" \
  || { echo "status does not show the queue" >&2; exit 1; }
echo "s90 OK"
