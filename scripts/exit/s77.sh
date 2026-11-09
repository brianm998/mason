#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s77 exit test: an agent's own `mason run` does not starve a task already in its test step.
# A real masond with the scripted agent and `compile` limited to 1. One task's agent (the
# first to start) queues `mason run --needs compile` six times in a row, from its develop
# step. The other task reaches `test`, queues for compile, and must be granted the slot next
# time it is free, well before the six runs are over. `mason resources` shows the agent's
# queued run as pipeline work (not interactive), and a `mason run` made by hand outside any
# task still counts as interactive.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
command -v python3 >/dev/null || { echo "s77 needs python3" >&2; exit 1; }
root=$(mktemp -d /tmp/mason-s77.XXXXXX); daemon=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  for f in "$root"/d/h/*/host.kv; do
    [ -e "$f" ] || continue
    for k in host_pid child_pid; do pid=$(sed -n "s/^$k=//p" "$f"); kill -9 "-$pid" 2>/dev/null || kill -9 "$pid" 2>/dev/null || true; done
  done
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=$PWD/target/debug
owner=${USER:-owner}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { set +e; echo "FAILED: $*" >&2; ls "$root" >&2; cat "$root/hog.log" >&2; m task list >&2 2>&1; m resources >&2 2>&1; tail -20 "$root/masond.log" >&2; exit 1; }
landed() { m task list --json | python3 -c "import json,sys; print(sum(1 for t in json.load(sys.stdin)['tasks'] if t.get('state')=='landed'))"; }

echo "== setup: a repository whose definition limits compile to 1, the scripted agent"
mkdir -p "$root/c/projects" "$root/repo"
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
echo base > "$root/repo/base.txt"
mkdir -p "$root/repo/.mason"
cat > "$root/repo/.mason/mason.toml" <<'TOML'
schema = 1
start = "develop"

[resource.compile]
count = 1

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
uses = { compile = 1 }
max_rounds = 2
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
TOML
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm init
git -C "$root/repo" push -q origin main
git -C "$root/repo" checkout -q -b owner-work
# The first agent to start is the one that keeps running tests; the other just commits.
ln -s "$bin" "$root/bin"   # the checkout path may hold spaces; the agent's shell line must not
cat > "$root/script.json" <<JSON
{"steps": {"develop": [[{"sh": "if mkdir $root/hog 2>/dev/null; then for i in 1 2 3 4 5 6; do $root/bin/mason --data-dir $root/d --config-dir $root/c run --needs compile --label \"tests \$MASON_TASK\" -- sh -c 'sleep 4; echo run >> $root/hog.log'; done; fi; f=\$(mktemp ./f-XXXXXX) && echo x > \$f && git add -A && git -c user.name=t -c user.email=t@t commit -qm change"}, {"call": {"tool": "submit_change", "args": {"summary": "done", "head": "\$HEAD"}}}]]}}
JSON
cat > "$root/c/projects/mason.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"
claude_bin = "$bin/fake-claude"
step_timeout_s = 300
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --max-concurrent 4 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== two tasks: one agent keeps queueing mason run, the other goes on to test"
m task add "one" --project mason --explored --body "x" >/dev/null
m task add "two" --project mason --explored --body "x" >/dev/null

echo "== the agent's queued run is shown as pipeline work, not interactive"
seen=""; end=$((SECONDS + 60))
until [ -n "$seen" ]; do
  [ $SECONDS -lt $end ] || fail "never saw the agent's run waiting for compile"
  seen=$(m resources --json | python3 -c "
import json,sys
w=[x for x in json.load(sys.stdin)['waiting'] if x['label'].startswith('tests ')]
print(w[0]['label'] if w and not w[0]['interactive'] else '')")
  sleep 0.2
done
echo "   waiting as '$seen', not interactive"
m resources | grep -q 'tests [0-9]* .*(interactive)' && fail "mason resources still calls it interactive"

echo "== the other task's test step is granted before the six runs are over"
end=$((SECONDS + 120))
until [ "$(landed)" -ge 1 ]; do [ $SECONDS -lt $end ] || fail "no task landed"; sleep 0.2; done
runs=$(wc -l < "$root/hog.log" | tr -d ' ')
[ "$runs" -lt 6 ] || fail "the first task's test step waited out all $runs runs"
echo "   a task landed after $runs of 6 runs"

echo "== both tasks land"
end=$((SECONDS + 150))
until [ "$(landed)" -ge 2 ]; do [ $SECONDS -lt $end ] || fail "both tasks did not land"; sleep 0.5; done
echo "   both landed"

echo "== a run made by hand, with no task, is still interactive"
m run --needs compile --label "by hand" -- sleep 4 2>/dev/null &
first=$!
sleep 1
m run --needs compile --label "by hand 2" -- true 2>/dev/null &
second=$!
ok=""; end=$((SECONDS + 20))
until [ -n "$ok" ]; do
  [ $SECONDS -lt $end ] || fail "the hand run was not seen waiting as interactive"
  ok=$(m resources --json | python3 -c "
import json,sys
w=[x for x in json.load(sys.stdin)['waiting'] if x['label']=='by hand 2']
print('y' if w and w[0]['interactive'] else '')")
  sleep 0.2
done
wait "$first" "$second"
echo "   waiting as interactive"

trap - EXIT; cleanup
echo "s77 OK"
