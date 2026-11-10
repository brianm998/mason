#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 114 exit test: a sound or spoken alert counts as delivered only when a monitor plays it.
# A real masond. A task fails with no monitor present; `deliveries` in /notify shows nothing
# played; a client then claims and is handed the failure; a repeat claim is handed nothing.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s114.XXXXXX); daemon=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
owner=${USER:-owner}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() {
  echo "FAIL: $*" >&2
  tail -20 "$root/masond.log" >&2
  exit 1
}
wait_for() {  # SECONDS WHAT CMD...
  local end=$((SECONDS + $1)) what=$2; shift 2
  until "$@" >/dev/null 2>&1; do [ $SECONDS -lt $end ] || fail "timed out waiting for $what"; sleep 0.3; done
}
has_alert() { m alerts | grep -q -- "$1"; }
queued() { curl -fsS "http://127.0.0.1:$port/notify" | python3 -I -c "
import json,sys
r=json.load(sys.stdin)
sys.exit(0 if any('failed' in x['kind'] for x in r['recent']) else 1)"; }
claim() { curl -fsS -X POST -H 'content-type: application/json' -d '{"client":"exit-monitor"}' "http://127.0.0.1:$port/announce/claim"; }

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
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 \
  --reload-ms 300 --fetch-interval-ms 300 --attention-ms 300 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
wait_for 20 "masond" m status
port=$(python3 -I -c "import json;print(json.load(open('$root/d/daemon.json'))['port'])")

echo "== 1. a task fails with no monitor present: queued, nothing played"
m task add "will fail" --project mason --explored --body "x" >/dev/null
wait_for 60 "the failed task's alert" has_alert "failed"
wait_for 30 "the failure to be queued" queued
curl -fsS "http://127.0.0.1:$port/notify" | python3 -I -c "
import json,sys
r=json.load(sys.stdin)
assert r['deliveries']==[], r['deliveries']
assert r['last_played'] is None, r['last_played']
print('   deliveries:', r['deliveries'])"

echo "== 2. a client claims and is handed the failure"
first=$(claim)
echo "$first" | python3 -I -c "
import json,sys
r=json.load(sys.stdin)
assert r['speaker'] is True, r
assert any(a['kind']=='failed' for a in r['announcements']), r
print('   handed', len(r['announcements']), 'announcement(s)')" || fail "the claim was not handed the failure: $first"
curl -fsS "http://127.0.0.1:$port/notify" | python3 -I -c "
import json,sys
r=json.load(sys.stdin)
assert {d['channel'] for d in r['deliveries'] if d['ok']}>={'sound','speech'}, r['deliveries']
assert r['last_played'] is not None
print('   deliveries now:', len(r['deliveries']))"

echo "== 3. a repeat claim is handed nothing"
second=$(claim)
echo "$second" | python3 -I -c "
import json,sys
r=json.load(sys.stdin)
assert r['announcements']==[], r" || fail "the repeat claim was handed something: $second"

trap - EXIT; cleanup
echo "s114 OK"
