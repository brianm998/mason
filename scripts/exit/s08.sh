#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 8 exit test: two projects at once with a contended machine class.
#  1. Admission follows the rank rule across projects (fewest steps remaining first).
#  2. The shared `compile` class is never held by two steps at once.
#  3. Waits explain themselves, in the form from the design.
#  4. An interactive `mason run --needs compile` goes ahead of pipeline work.
#  5. A killed lock holder frees its resource and the next waiter starts at once.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s08.XXXXXX); daemon=""; holder=""
cleanup() {
  for p in $holder; do kill -9 "$p" 2>/dev/null || true; done
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  for f in "$root"/d/h/*/host.kv; do
    [ -e "$f" ] || continue
    for k in host_pid child_pid; do pid=$(sed -n "s/^$k=//p" "$f"); kill -9 "-$pid" 2>/dev/null || kill -9 "$pid" 2>/dev/null || true; done
  done
  rm -rf "$root"
}
trap cleanup EXIT

cargo build -q --workspace --bins
bin=target/debug
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
ms() { perl -MTime::HiRes=time -e 'printf "%d\n", time*1000'; }
jq_state() { sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
wait_state() {  # TASK STATE SECONDS
  local end=$((SECONDS + $3))
  until [ "$(m task show "$1" --json 2>/dev/null | jq_state)" = "$2" ]; do
    [ $SECONDS -lt $end ] || { echo "timed out waiting for task $1 to be $2" >&2; m task show "$1" >&2 || true; exit 1; }
    sleep 0.2
  done
}
wait_waiting() {  # TASK SECONDS: prints the task's wait sentence
  local end=$((SECONDS + $2)) w=""
  while :; do
    w=$(m task show "$1" 2>/dev/null | sed -n 's/^  waiting   //p')
    [ -n "$w" ] && { echo "$w"; return; }
    [ $SECONDS -lt $end ] || { echo "task $1 never showed a wait" >&2; m task show "$1" >&2 || true; exit 1; }
    sleep 0.2
  done
}
task_id() { sed -n 's/.*#\([0-9]*\).*/\1/p'; }
first_start() { m task show "$1" --json | grep -m1 '"started_at"' | sed 's/[^0-9]//g'; }
pusher() {  # commit a file to a branch of origin as someone else would
  [ -d "$root/pusher" ] || git clone -q "$root/origin.git" "$root/pusher" 2>/dev/null
  git -C "$root/pusher" fetch -q origin
  git -C "$root/pusher" checkout -q -B "$1" "origin/$1"
  mkdir -p "$root/pusher/$(dirname "$2")"; cat > "$root/pusher/$2"
  git -C "$root/pusher" add -A
  git -C "$root/pusher" -c user.name=t -c user.email=t@t commit -qm "$3"
  git -C "$root/pusher" push -q origin "HEAD:$1"
}

# One repository with two project branches: alpha on main, beta on its own.
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
echo scratch > "$root/repo/README"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm init
git -C "$root/repo" push -q origin main
git -C "$root/repo" push -q origin main:beta
git -C "$root/repo" checkout -q -b owner-work
mkdir -p "$root/c/projects"
cat > "$root/script.json" <<'JSON'
{"steps": {
  "explore": [[{"call": {"tool": "submit_plan", "args": {"summary": "add hello.txt", "files": ["hello.txt"], "approach": "write the file", "acceptance_criteria": ["hello.txt exists"]}}}]],
  "develop": [[
    {"sh": "echo hello > hello.txt && echo $PWD > unique-$(basename \"$PWD\").txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm 'add hello'"},
    {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "$HEAD"}}}
  ]]
}}
JSON
# The test command records when it holds the compile class, so overlap would show.
cat > "$root/testcmd.sh" <<SH
#!/bin/sh
now() { perl -MTime::HiRes=time -e 'printf "%d\n", time*1000'; }
t=\$(basename "\$PWD"); p=\$(basename "\$(dirname "\$PWD")")
echo "\$(now) start \$p/\$t" >> "$root/compile.log"
sleep 1.5
test -f hello.txt; r=\$?
echo "\$(now) end \$p/\$t" >> "$root/compile.log"
exit \$r
SH
for p in alpha:main beta:beta; do
  cat > "$root/c/projects/${p%%:*}.toml" <<TOML
path = "$root/repo"
base = "${p##*:}"
remote = "origin"
test = "sh $root/testcmd.sh"
claude_bin = "$PWD/$bin/fake-claude"
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML
done
# alpha explores first (4 steps to landed); beta does not (3). Both test under `compile`.
pusher main .mason/mason.toml "alpha definition" <<'TOML'
schema = 1
start = "explore"

[resource.compile]
count = 1

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
uses = { compile = 1 }
next = "land"

[step.land]
type = "landing"
category = "landing"
TOML
pusher beta .mason/mason.toml "beta definition" <<'TOML'
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
next = "land"

[step.land]
type = "landing"
category = "landing"
TOML

"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --max-concurrent 2 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== 1. admission follows the rank rule across projects"
m resources limit llm:claude 0 | head -1
a=$(m task add "Long graph, filed first" --project alpha --body "x" | task_id)
sleep 0.1
b=$(m task add "Short graph, filed second" --project beta --body "x" | task_id)
sleep 1
[ "$(m task show "$a" --json | jq_state)" = queued ] && [ "$(m task show "$b" --json | jq_state)" = queued ] \
  || { echo "no task may be claimed while no LLM session is free" >&2; exit 1; }
m resources | grep -E 'llm:claude|Waiting' || true
m resources limit llm:claude 1 | head -1
wait_state "$a" landed 120; wait_state "$b" landed 120
sa=$(first_start "$a"); sb=$(first_start "$b")
echo "alpha #$a (4 steps) started at $sa, beta #$b (3 steps) at $sb"
[ "$sb" -lt "$sa" ] || { echo "the task nearer to landing must be admitted first" >&2; exit 1; }

echo "== 2. the compile class is never held twice at once"
# start/end must strictly alternate in the log.
awk '$2=="start"{ if (open) { print "overlap at " $0; bad=1 } open=1 } $2=="end"{ open=0 } END{ exit bad }' "$root/compile.log"
echo "$(grep -c start "$root/compile.log") test steps held it one at a time"

echo "== 3 and 4 and 5. waits explain themselves; interactive goes first; a killed holder frees it"
m resources limit llm:claude 2 | head -1
: > "$root/compile.log"
# Started directly (not through the `m` function) so $! is the mason process itself.
"$bin/mason" --data-dir "$root/d" --config-dir "$root/c" run --needs compile --label "interactive holder" -- sleep 300 > /dev/null 2>&1 &
holder=$!
until m resources | grep -q 'held by interactive holder'; do sleep 0.2; done
echo "a long interactive job holds compile"
ta=$(m task add "Waits for compile (alpha)" --project alpha --body "x" | task_id)
tb=$(m task add "Waits for compile (beta)" --project beta --body "x" | task_id)
wa=$(wait_waiting "$ta" 60); wb=$(wait_waiting "$tb" 60)
echo "  #$ta: $wa"; echo "  #$tb: $wb"
# Whichever is first in line is held up by the holder; the other is behind it.
first="$wa"; second="$wb"; case "$wa" in *"2nd in line"*) first="$wb"; second="$wa";; esac
echo "$first"  | grep -q '^Waiting for compile slot: 1st in line, held by interactive holder for '
echo "$second" | grep -Eq '^Waiting for compile slot: 2nd in line, behind #[0-9]+ \(2 steps from landing\), held by interactive holder for '
# A second interactive caller arrives later but goes ahead of both tasks.
"$bin/mason" --data-dir "$root/d" --config-dir "$root/c" run --needs compile --label "interactive latecomer" -- \
  sh -c "perl -MTime::HiRes=time -e 'printf \"%d\\n\", time*1000' > '$root/i.time'; echo ran > '$root/i.out'" > /dev/null 2>&1 &
until m resources | grep -q 'interactive latecomer'; do sleep 0.2; done
m resources | sed -n '/^Waiting/,$p'
m resources | sed -n '/^Waiting/,$p' | sed -n 2p | grep -q 'interactive latecomer' \
  || { echo "the interactive caller should be first in line" >&2; exit 1; }
# Kill the holder outright. Its locks vanish with it.
t0=$(ms)
orphans=$(pgrep -P "$holder" 2>/dev/null || true)
kill -9 "$holder"; for c in $orphans; do kill -9 "$c" 2>/dev/null || true; done
until [ -e "$root/i.out" ]; do
  [ $(( $(ms) - t0 )) -lt 10000 ] || { echo "the next waiter did not start after the holder was killed" >&2; m resources >&2; exit 1; }
  sleep 0.05
done
took=$(( $(ms) - t0 ))
echo "the killed holder's resource was reused after ${took} ms"
[ "$took" -lt 5000 ]
wait_state "$ta" landed 120; wait_state "$tb" landed 120
ti=$(cat "$root/i.time")
first_task_start=$(awk '$2=="start"{print $1; exit}' "$root/compile.log")
[ "$ti" -lt "$first_task_start" ] || { echo "the interactive caller should have run before pipeline work" >&2; exit 1; }
awk '$2=="start"{ if (open) { print "overlap at " $0; bad=1 } open=1 } $2=="end"{ open=0 } END{ exit bad }' "$root/compile.log"
m resources | grep -E '^  compile ' | grep -q ' 0/1 in use' || { m resources >&2; echo "compile was not released" >&2; exit 1; }
echo "everything landed, nothing is still held"

# The rest: the scheduler's property tests (ordering, no deadlock, no lost lease),
# the kernel-lock tests, and the daemon's scheduling tests.
trap - EXIT; cleanup
if [ -z "${MASON_TESTS_ALREADY_RAN:-}" ]; then
  cargo test -p mason-scheduler -p mason-resources
  cargo test -p mason-daemon --test scheduling
fi
echo "s08 OK"
