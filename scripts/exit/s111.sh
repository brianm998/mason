#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 111 exit test: a hung agent is noticed. A real masond with the scripted agent
# (fake-claude) and short limits: the agent prints and goes quiet, the silence alert appears,
# the session is stopped after the stop limit and resumed, the task lands, and the alert is gone.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s111.XXXXXX); daemon=""
stop_daemon() {
  if [ -n "$daemon" ]; then kill -9 "$daemon" 2>/dev/null || true; wait "$daemon" 2>/dev/null || true; fi
  daemon=""
}
kill_hosts() {
  if [ -d "$root/d/h" ]; then
    target/debug/step-host-ctl reap "$root/d/h" >/dev/null 2>&1 || echo "note: host reap reported an error" >&2
  fi
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
    [{"say": "going-quiet"}, {"sh": "sleep 120"}],
    [
      {"sh": "echo hello > f-$(date +%s%N).txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm hello"},
      {"call": {"tool": "submit_change", "args": {"summary": "added a file", "head": "$HEAD"}}}
    ]
  ]
}}
JSON
printf 'echo tests ok\n' > "$root/t.sh"
cat > "$cdir/projects/p.toml" <<TOML
path = "$repo"
base = "main"
remote = "origin"
test = "sh $root/t.sh"
claude_bin = "$PWD/$bin/fake-claude"
agent_idle_alert_s = 2
agent_idle_stop_s = 10
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML
: > "$root/masond.log"
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$cdir" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== a quiet agent: alert, then stopped and resumed, and the task lands"
m task add "first" --project p --body x >/dev/null
seen=""
end=$((SECONDS + 60))
until [ -n "$seen" ]; do
  alerts_out=$(m alerts 2>/dev/null) || alerts_out=""
  if grep -qF 'agent in `develop` has been silent for' <<<"$alerts_out"; then
    seen=$(grep -F 'agent in `develop` has been silent for' <<<"$alerts_out")
  fi
  [ $SECONDS -lt $end ] || die "no silence alert appeared"
  sleep 0.3
done
echo "   $seen"
grep -q "Task #0" <<<"$seen" && grep -q '`develop`' <<<"$seen" || die "the alert does not name the task and step: $seen"
wait_state 0 landed 120
j=$(m task show 0 --json)
[ "$(pyget failures <<<"$j")" = 0 ] || die "a silent stop counted as a failure: $j"
grep -q -- "--resume" "$root/script.json.argv" || die "the step was not resumed"
grep -q "silence" <<<"$(pyget host_losses.0.detail <<<"$j")" || die "the stop is not recorded: $j"
if m alerts | grep -qF "has been silent for"; then m alerts >&2; die "the alert did not clear"; fi
echo "s111 passed"
