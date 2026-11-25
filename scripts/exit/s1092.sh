#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 1092 exit test: the review agent is told the task's reserved stage note and decision
# record. A real masond with the scripted agent (fake-claude); no model, no network. The review
# step's prompt carries the paths `mason task show` lists, and review reserves nothing more.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s1092.XXXXXX); daemon=""
stop_daemon() {
  if [ -n "$daemon" ]; then kill -9 "$daemon" 2>/dev/null || true; wait "$daemon" 2>/dev/null || true; fi
  daemon=""
}
kill_hosts() {
  # Verified before it is killed: a stale pid in a registry may belong to someone else now.
  for hd in "$root"/d/h; do target/debug/step-host-ctl reap "$hd" >/dev/null 2>&1; done
}
cleanup() { stop_daemon; kill_hosts; rm -rf "$root" 2>/dev/null || { sleep 1; rm -rf "$root"; }; }
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
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "review"

[step.review]
type = "agent"
category = "review"
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
  "develop": [[
    {"sh": "echo one > f1.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm one"},
    {"call": {"tool": "submit_change", "args": {"summary": "added a file", "head": "$HEAD"}}}
  ]],
  "review": [[
    {"sh": "printf %s \"$FAKE_PROMPT\" > $REVIEW_OUT"},
    {"call": {"tool": "submit_review", "args": {"verdict": "approve", "summary": "fine"}}}
  ]]
}}
JSON
cat > "$cdir/projects/p.toml" <<TOML
path = "$repo"
base = "main"
remote = "origin"
test = "true"
claude_bin = "$PWD/$bin/fake-claude"
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
REVIEW_OUT = "$root/review-prompt"
TOML
: > "$root/masond.log"
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$cdir" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== the review prompt names the task's reserved files"
m task add "first" --project p --body x >/dev/null
wait_state 0 landed 180
[ -s "$root/review-prompt" ] || die "the review prompt was not captured"
m task show 0 > "$root/show"
numbers=$(grep -E '^ *number ' "$root/show") || { cat "$root/show" >&2; die "task show lists no numbers"; }
[ "$(printf '%s\n' "$numbers" | wc -l | tr -d ' ')" = 2 ] || die "expected one exit and one decision number: $numbers"
note=$(printf '%s\n' "$numbers" | grep -oE 'docs/stages/s[0-9]+\.md') || die "no stage note in: $numbers"
dec=$(printf '%s\n' "$numbers" | grep -oE 'docs/decisions/[0-9]{4}-<slug>\.md') || die "no decision record in: $numbers"
for want in "stage note $note" "decision record $dec" "not scope creep"; do
  grep -qF "$want" "$root/review-prompt" || { cat "$root/review-prompt" >&2; die "the review prompt lacks: $want"; }
done
echo "s1092 passed"
