#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 63 exit test: the tier and the real model in use, `deep (Opus medium)`, wherever a model is
# shown. A real masond with the scripted agent (fake-claude); no model, no network.
#  1. a running step's task row, `mason status` and the home page show `deep (<model> <effort>)`
#     taken from the provider mapping, and `mason task show --json` carries the parts.
#  2. changing the provider file changes what a new step shows (expected, then launched) and does
#     not change what the already-running step shows.
#  3. a ladder retry shows the higher model and `retry 1 of 2`; the run's history has both launches.
#  4. a runtime override is marked `override`.
#  5. a restarted daemon shows the same string for the step it reattached and writes no new launch.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s63.XXXXXX); daemon=""
stop_daemon() {
  if [ -n "$daemon" ]; then kill -9 "$daemon" 2>/dev/null || true; wait "$daemon" 2>/dev/null || true; fi
  daemon=""
}
kill_hosts() {
  for f in "$root"/d/h/*/host.kv; do
    [ -e "$f" ] || continue
    for k in host_pid child_pid; do pid=$(sed -n "s/^$k=//p" "$f"); kill -9 "-$pid" 2>/dev/null || kill -9 "$pid" 2>/dev/null || true; done
  done
}
cleanup() { stop_daemon; kill_hosts; rm -rf "$root"; }
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
owner=${USER:-owner}
cdir="$root/c"; mkdir -p "$cdir/projects" "$cdir/providers"
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$cdir" "$@"; }
# pyget FIELD-PATH < json: print a (dotted) field of the JSON on stdin
pyget() { python3 -I -c 'import json,sys; v=json.load(sys.stdin)
for k in sys.argv[1].split("."):
    v = v[int(k)] if isinstance(v, list) else v.get(k) if isinstance(v, dict) else None
print("" if v is None else (json.dumps(v) if isinstance(v,(dict,list,bool)) else v))' "$1"; }
disp() { m task show "$1" --json | pyget model.display; }
expect() {  # WHAT GOT WANT
  [ "$2" = "$3" ] || { echo "$1: got [$2], wanted [$3]" >&2; tail -20 "$root/masond.log" >&2; exit 1; }
  echo "   $1: $2"
}
wait_disp() {  # TASK WANT SECONDS: wait for the display of a task to be exactly WANT
  local end=$((SECONDS + $3)) got=""
  until got=$(disp "$1" 2>/dev/null) && [ "$got" = "$2" ]; do
    [ $SECONDS -lt $end ] || { echo "task $1 shows [$got], wanted [$2]" >&2; tail -20 "$root/masond.log" >&2; exit 1; }
    sleep 0.2
  done
  echo "   task $1 shows: $got"
}
wait_state() {  # TASK STATE SECONDS
  local end=$((SECONDS + $3))
  until [ "$(m task show "$1" --json 2>/dev/null | pyget state)" = "$2" ]; do
    [ $SECONDS -lt $end ] || { echo "timed out waiting for task $1 to be $2" >&2; m task show "$1" >&2 || true; tail -20 "$root/masond.log" >&2; exit 1; }
    sleep 0.3
  done
}
launches() {  # TASK: the launch rows of the task's first step run
  m task show "$1" --json | python3 -I -c 'import json,sys
print(len(json.load(sys.stdin)["steps"][0]["launches"]))'; }
home_active() {  # the home page's active-model items, from the daemon's own view
  local port; port=$(sed -n 's/.*"port": *\([0-9]*\).*/\1/p' "$root/d/daemon.json")
  rm -rf "$root/items"; "$bin/conformance" expected --port "$port" --out "$root/items" --now >/dev/null
  python3 -I -c 'import json,sys
for i in json.load(open(sys.argv[1] + "/checkpoint-0.json")):
    if i["id"].startswith("active:"): print(i["id"], "|", i["texts"][0])' "$root/items"
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
agent = "deep"
effort = "medium"
next = "develop"

[step.develop]
type = "agent"
category = "developing"
agent = "light"
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

# explore sleeps (so the step is visible while it runs); turn 1 of the first task crashes, which
# is the ladder retry; develop is quick.
write_script() {  # SECONDS-TO-SLEEP FIRST-TURN-CRASHES(yes|no)
  rm -f "$root/script.json"*
  local first='{"sleep_ms": '$(( $1 * 1000 ))'}, {"call": {"tool": "submit_plan", "args": {"summary": "add hello.txt", "files": ["hello.txt"], "approach": "write the file", "acceptance_criteria": ["hello.txt exists"]}}}'
  local turns="[$first]"
  [ "$2" = yes ] && turns='[{"exit": 1}],['"$first"']'
  cat > "$root/script.json" <<JSON
{"steps": {
  "explore": [$turns],
  "develop": [[
    {"sh": "echo hello > f-\$(date +%s%N).txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm hello"},
    {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "\$HEAD"}}}
  ]]
}}
JSON
}
cat > "$cdir/projects/p.toml" <<TOML
path = "$repo"
base = "main"
remote = "origin"
test = "ls f-*.txt"
provider = "claude"
claude_bin = "$PWD/$bin/fake-claude"
ladder = ["haiku", "sonnet", "opus"]
max_retries = 2
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML
cat > "$cdir/providers/claude.toml" <<'TOML'
[deep]
medium = { model = "sonnet", effort = "medium" }
TOML
launch_daemon() {
  "$bin/masond" --data-dir "$root/d" --config-dir "$cdir" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
  daemon=$!
  until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
  m resources limit llm:claude 1 >/dev/null || { m resources >&2; exit 1; }   # one session at a time: a second step waits for it
}
: > "$root/masond.log"

echo "== 1. a running step shows the tier and the real model from the provider mapping"
write_script 20 no
launch_daemon
m task add "first" --project p --body x >/dev/null
wait_disp 0 "deep (Sonnet medium)" 30
m status | grep -F "active: #0 explore on deep (Sonnet medium)" >/dev/null \
  || { m status >&2; echo "mason status does not show the display" >&2; exit 1; }
echo "   mason status says the same"
expect "home page" "$(home_active)" "active:0 | deep (Sonnet medium)"
j=$(m task show 0 --json)
for kv in model.tier=deep model.level=medium model.model=sonnet model.effort=medium model.provider=claude model.expected=false; do
  expect "show --json ${kv%%=*}" "$(pyget "${kv%%=*}" <<<"$j")" "${kv#*=}"
done
case "$(pyget model.why <<<"$j")" in *"provider file"*) echo "   why: $(pyget model.why <<<"$j")";; *) echo "why does not name the provider file" >&2; exit 1;; esac
grep -qE -- "--model sonnet --effort medium" "$root/script.json.argv"
echo "   the CLI was launched with exactly that model and effort"

echo "== 2. a changed provider file changes a new step, not the running one"
cat > "$cdir/providers/claude.toml" <<'TOML'
[deep]
medium = { model = "opus", effort = "high" }
[light]
medium = { model = "sonnet", effort = "low" }
TOML
m task add "second" --project p --body x >/dev/null   # one session at a time: it waits for task 0
expect "the running step keeps its string" "$(disp 0)" "deep (Sonnet medium)"
expect "launches written for the running step" "$(launches 0)" "1"
# task 0 finishes and frees the session; task 1 then starts and shows the mapping as it is now
wait_disp 1 "deep (Opus high)" 120
j=$(m task show 0 --json)
expect "task 0 explore run keeps" "$(pyget steps.0.launches.0.display <<<"$j")" "deep (Sonnet medium)"
expect "a new step's launch" "$(m task show 1 --json | pyget steps.0.launches.0.display)" "deep (Opus high)"
grep -qE -- "--model opus --effort high" "$root/script.json.argv"
wait_state 1 landed 120
wait_state 0 landed 120

echo "== 3. a ladder retry shows the higher model and the retry count"
cat > "$cdir/providers/claude.toml" <<'TOML'
[deep]
medium = { model = "sonnet", effort = "medium" }
TOML
write_script 12 yes
m task add "third" --project p --body x >/dev/null
wait_disp 2 "deep (Opus medium, retry 1 of 2)" 60
j=$(m task show 2 --json)
expect "first launch" "$(pyget steps.0.launches.0.display <<<"$j")" "deep (Sonnet medium)"
expect "retry launch" "$(pyget steps.0.launches.1.display <<<"$j")" "deep (Opus medium, retry 1 of 2)"
expect "retry kind" "$(pyget steps.0.launches.1.kind <<<"$j")" "retry"
wait_state 2 landed 90

echo "== 4. a runtime override is marked"
write_script 25 no
m config set step.explore.model opus --for 1h --owner brian --project p >/dev/null
m task add "fourth" --project p --body x >/dev/null
wait_disp 3 "deep (Opus medium, override)" 60
expect "show --json overridden" "$(m task show 3 --json | pyget model.overridden)" "true"

echo "== 5. a restarted daemon shows the same string and writes no new launch"
cat > "$cdir/providers/claude.toml" <<'TOML'
[deep]
medium = { model = "haiku", effort = "low" }
TOML
before=$(launches 3)
cli_starts=$(wc -l < "$root/script.json.argv")
stop_daemon
launch_daemon
sleep 3
expect "agent CLI starts (none new)" "$(wc -l < "$root/script.json.argv" | tr -d ' ')" "$(echo $cli_starts)"
expect "after the restart" "$(disp 3)" "deep (Opus medium, override)"
expect "launch rows of the run" "$(launches 3)" "$before"
wait_state 3 landed 120
expect "launch rows of the run after it finished" "$(launches 3)" "$before"

stop_daemon
trap - EXIT; cleanup
echo "s63 OK"
