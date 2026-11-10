#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 88 exit test: alerts keep working while git work is stuck. A real masond started with
# --debug and a short "stuck" limit; the repository lock is held through /debug/hold-repo-lock
# (the same lock a hung fetch would hold).
#  1. A task fails; `mason alerts` shows its alert while the repository lock is held.
#  2. After the limit the "git work is stuck" alert appears, naming what holds the lock.
#  3. When the lock is released the stuck alert clears and the task's alert is still there.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s88.XXXXXX); daemon=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  for hd in "$root"/d/h; do target/debug/step-host-ctl reap "$hd" >/dev/null 2>&1; done
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
owner=${USER:-owner}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() {
  echo "FAIL: $*" >&2
  if ! m alerts >&2; then echo "(mason alerts failed too)" >&2; fi
  tail -20 "$root/masond.log" >&2
  exit 1
}
wait_for() {  # SECONDS WHAT CMD...
  local end=$((SECONDS + $1)) what=$2; shift 2
  until "$@" >/dev/null 2>&1; do [ $SECONDS -lt $end ] || fail "timed out waiting for $what"; sleep 0.3; done
}
has_alert() { m alerts | grep -q -- "$1"; }
lacks_alert() { ! m alerts | grep -q -- "$1"; }

git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
echo hi > "$root/repo/README"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm init
git -C "$root/repo" push -q origin main
mkdir -p "$root/c/projects"
echo '{"steps": {"develop": [[{"sh": "exit 1"}]]}}' > "$root/script.json"
cat > "$root/c/projects/mason.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"
claude_bin = "$PWD/$bin/fake-claude"
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML
"$bin/masond" --port 0 --debug --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 \
  --reload-ms 300 --fetch-interval-ms 300 --attention-ms 300 --stuck-after-ms 3000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
wait_for 20 "masond" m status
port=$(python3 -I -c "import json;print(json.load(open('$root/d/daemon.json'))['port'])")

echo "== 1. a failed task's alert, while the repository lock is held"
m task add "will fail" --project mason --explored --body "x" >/dev/null
curl -fsS -X POST "http://127.0.0.1:$port/debug/hold-repo-lock?ms=12000" >/dev/null || fail "no debug route"
wait_for 60 "the failed task's alert" has_alert "Task #0 failed"
echo "   mason alerts shows: $(m alerts | grep 'Task #0 failed')"

echo "== 2. the stuck alert appears and names the holder"
wait_for 30 "the stuck alert" has_alert "Git work is stuck"
m alerts | grep "Git work is stuck" | grep -q "held by http.rs" || fail "the holder is not named"
echo "   $(m alerts | grep 'Git work is stuck')"

echo "== 3. it clears when the lock is released"
wait_for 40 "the stuck alert to clear" lacks_alert "Git work is stuck"
has_alert "Task #0 failed" || fail "the task's alert went away"

trap - EXIT; cleanup
echo "s88 OK"
