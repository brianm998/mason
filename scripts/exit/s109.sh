#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 109 exit test: an approval wait raises an alert, and approving clears it.
# A real masond on the human-gated template with the scripted agent (fake-claude); no model, no network.
# `mason alerts` shows the approval wait as needs_you; `mason task approve` clears it and the task goes on
# to the next approval, which is an alert of its own, and then lands.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s109.XXXXXX); daemon=""
stop_daemon() {
  if [ -n "$daemon" ]; then kill -9 "$daemon" 2>/dev/null || true; wait "$daemon" 2>/dev/null || true; fi
  daemon=""
}
kill_hosts() {
  # Verified before it is killed: a stale pid in a registry may belong to someone else now.
  for hd in "$root"/d/h; do target/debug/step-host-ctl reap "$hd" >/dev/null 2>&1; done
}
cleanup() { stop_daemon; kill_hosts; rm -rf "$root"; }
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
owner=${USER:-owner}
cdir="$root/c"; mkdir -p "$cdir/projects"
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$cdir" "$@"; }
pyget() { python3 -I -c 'import json,sys; v=json.load(sys.stdin)
for k in sys.argv[1].split("."):
    v = (v[int(k)] if int(k) < len(v) else None) if isinstance(v, list) else v.get(k) if isinstance(v, dict) else None
print("" if v is None else (json.dumps(v) if isinstance(v,(dict,list,bool)) else v))' "$1"; }
die() { echo "$1" >&2; tail -20 "$root/masond.log" >&2; exit 1; }
wait_state() {  # TASK STATE SECONDS
  local end=$((SECONDS + $3))
  until [ "$(m task show "$1" --json 2>/dev/null | pyget state)" = "$2" ]; do
    [ $SECONDS -lt $end ] || { m task show "$1" >&2; die "timed out waiting for task $1 to be $2"; }
    sleep 0.3
  done
}
repo="$root/r"; origin="$root/o.git"
git init -q --bare -b main "$origin" 2>/dev/null
git clone -q "$origin" "$repo" 2>/dev/null
git -C "$repo" checkout -q -b main
mkdir -p "$repo/.mason"
sed 's|^# suites: mason init fills this in||' templates/human-gated/mason.toml > "$repo/.mason/mason.toml"
cp templates/human-gated/landing.toml "$repo/.mason/landing.toml"
echo base > "$repo/README"
git -C "$repo" add -A
git -C "$repo" -c user.name=t -c user.email=t@t commit -qm pipeline
git -C "$repo" push -q origin main
git -C "$repo" checkout -q -b owner-work

cat > "$root/script.json" <<'JSON'
{"steps": {
  "explore": [[{"call": {"tool": "submit_plan", "args": {"summary": "add a file", "files": ["f.txt"], "approach": "write it", "acceptance_criteria": ["f.txt exists"]}}}]],
  "develop": [[
    {"sh": "echo hello > f.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm hello"},
    {"call": {"tool": "submit_change", "args": {"summary": "added a file", "head": "$HEAD"}}}
  ]]
}}
JSON
printf 'echo tests ok\n' > "$root/t.sh"
cat > "$cdir/projects/p.toml" <<TOML
path = "$repo"
base = "main"
remote = "origin"
test = "sh $root/t.sh"
claude_bin = "$PWD/$bin/fake-claude"
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML
: > "$root/masond.log"
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$cdir" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

wait_alert() {  # KEY SECONDS: wait until `mason alerts` lists KEY as needs_you with the given words
  local end=$((SECONDS + $2))
  until m alerts | grep -E "^! needs_you +.*waits for your approval.*\[$1\]" >/dev/null; do
    [ $SECONDS -lt $end ] || { m alerts >&2; die "no approval alert $1"; }
    sleep 0.3
  done
}
wait_clear() {  # KEY SECONDS
  local end=$((SECONDS + $2))
  while m alerts | grep -qF "[$1]"; do
    [ $SECONDS -lt $end ] || { m alerts >&2; die "alert $1 did not clear"; }
    sleep 0.3
  done
}

echo "== a task at the plan approval raises an alert; approving clears it"
m task add "first" --project p --body x >/dev/null
wait_alert approval:0 120
m alerts
m task approve 0 >/dev/null
wait_clear approval:0 60
echo "== the task goes on to the change approval, an alert of its own"
wait_alert approval:0 120
m task approve 0 >/dev/null
wait_state 0 landed 120
wait_clear approval:0 60
echo "s109 passed"
