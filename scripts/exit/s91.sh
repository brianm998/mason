#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 91 exit test: bytes that are not UTF-8 in a test command's output do not hide it.
# A real masond with the scripted agent (fake-claude); no model, no network.
# The test command prints a \377 byte and fails: the agent's next prompt carries the
# real output after it. The rerun prints it and passes: the task lands.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s91.XXXXXX); daemon=""
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
cat > "$repo/.mason/mason.toml" <<'TOML'
schema = 1
start = "explore"

[step.explore]
type = "agent"
category = "exploring"
next = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
max_rounds = 2
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
TOML
printf 'mode = "direct"\n' > "$repo/.mason/landing.toml"
echo base > "$repo/README"
git -C "$repo" add -A
git -C "$repo" -c user.name=t -c user.email=t@t commit -qm pipeline
git -C "$repo" push -q origin main
git -C "$repo" checkout -q -b owner-work

cat > "$root/script.json" <<'JSON'
{"steps": {
  "explore": [[{"call": {"tool": "submit_plan", "args": {"summary": "add a file", "files": ["f.txt"], "approach": "write it", "acceptance_criteria": ["f.txt exists"]}}}]],
  "develop": [
    [
      {"sh": "echo one > f1.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm one"},
      {"call": {"tool": "submit_change", "args": {"summary": "added a file", "head": "$HEAD"}}}
    ],
    [
      {"sh": "printf %s \"$FAKE_PROMPT\" > $PROMPT2_OUT; echo two > f2.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm two"},
      {"call": {"tool": "submit_change", "args": {"summary": "fixed it", "head": "$HEAD"}}}
    ]
  ]
}}
JSON
# Fails the first time, passes the second; both print an invalid byte first.
cat > "$root/t.sh" <<SH
printf 'noise \\377 in the log\\n'
if [ -e "$root/seen" ]; then echo tests ok; exit 0; fi
touch "$root/seen"
echo "REAL-FAILURE-TEXT-91"
exit 1
SH
cat > "$cdir/projects/p.toml" <<TOML
path = "$repo"
base = "main"
remote = "origin"
test = "sh $root/t.sh"
claude_bin = "$PWD/$bin/fake-claude"
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
PROMPT2_OUT = "$root/prompt2"
TOML
: > "$root/masond.log"
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$cdir" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== a test that prints invalid UTF-8 and fails, then passes"
m task add "first" --project p --body x >/dev/null
wait_state 0 landed 180
[ -s "$root/prompt2" ] || die "the second develop prompt was not captured"
grep -q "REAL-FAILURE-TEXT-91" "$root/prompt2" || { cat "$root/prompt2" >&2; die "the agent's feedback lacks the test output"; }
echo "s91 passed"
