#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 9 exit test (this proves milestone M2): the testing system end to end.
#  1. Two projects at once, in the correct order, sharing one `compile` class.
#  2. A vacuous pass is caught and goes back to development.
#  3. A flaky failure is classified on rerun, and the step still passes.
#  4. A baseline failure is excused (it fails at the merge base too).
#  5. A force skip stops a running suite, keeps the green lane and records a debt.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s09.XXXXXX); daemon=""
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
bin=target/debug
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
jq_state() { sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
wait_state() {  # TASK STATE SECONDS
  local end=$((SECONDS + $3))
  until [ "$(m task show "$1" --json 2>/dev/null | jq_state)" = "$2" ]; do
    [ $SECONDS -lt $end ] || { echo "timed out waiting for task $1 to be $2" >&2; m task show "$1" >&2 || true; exit 1; }
    sleep 0.2
  done
}
steps_of() { m task show "$1" --json | grep '"name":' | sed 's/.*"name": "\(.*\)".*/\1/' | tr '\n' ' ' | sed 's/ $//'; }
first_start() { m task show "$1" --json | grep -m1 '"started_at"' | sed 's/[^0-9]//g'; }
pusher() {  # BRANCH PATH MESSAGE: commit stdin to a branch of origin as someone else would
  [ -d "$root/pusher" ] || git clone -q "$root/origin.git" "$root/pusher" 2>/dev/null
  git -C "$root/pusher" fetch -q origin
  git -C "$root/pusher" checkout -q -B "$1" "origin/$1"
  mkdir -p "$root/pusher/$(dirname "$2")"; cat > "$root/pusher/$2"
  git -C "$root/pusher" add -A
  git -C "$root/pusher" -c user.name=t -c user.email=t@t commit -qm "$3"
  git -C "$root/pusher" push -q origin "HEAD:$1"
}
next=0
add() {  # PROJECT MODE TITLE: file a task, telling the scripted agent and suite how to behave
  # (not run in a subshell: the counter must survive; the id comes back in $LAST)
  LAST=$next; next=$((next + 1))
  echo "$2" > "$root/b/$LAST"
  m task add "$3" --project "$1" --body "x" > /dev/null
}

# One repository, two project branches (alpha on main, beta on its own).
mkdir -p "$root/b"
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

# The scripted agent does what the task's mode says (one scripted turn per
# develop invocation, so there are plenty). A marker outside the
# worktree tells its second attempt from its first.
cat > "$root/agent.sh" <<SH
n=\$(basename "\$PWD"); p=\$(basename "\$(dirname "\$PWD")")
mode=\$(cat "$root/b/\$n" 2>/dev/null || echo ok)
tried="\$PWD/../.tried-\$n"
first=1; [ -f "\$tried" ] && first=0; touch "\$tried"
echo "\$p/\$n first=\$first mode=\$mode" >> "$root/b/agent.log"
echo hello > hello.txt; echo "\$n" > "unique-\$p-\$n.txt"
case "\$mode" in
  vacuous) if [ \$first = 1 ]; then echo x > skip.flag; else rm -f skip.flag; fi ;;
esac
git add -A; git -c user.name=t -c user.email=t@t commit -qm "work on \$n"
SH
cat > "$root/script.json" <<JSON
{"steps": {
  "explore": [[{"call": {"tool": "submit_plan", "args": {"summary": "add hello.txt", "files": ["hello.txt"], "approach": "write the file", "acceptance_criteria": ["hello.txt exists"]}}}]],
  "develop": [
    [{"sh": "sh $root/agent.sh"}, {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "\$HEAD"}}}],
    [{"sh": "sh $root/agent.sh"}, {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "\$HEAD"}}}],
    [{"sh": "sh $root/agent.sh"}, {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "\$HEAD"}}}],
    [{"sh": "sh $root/agent.sh"}, {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "\$HEAD"}}}],
    [{"sh": "sh $root/agent.sh"}, {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "\$HEAD"}}}],
    [{"sh": "sh $root/agent.sh"}, {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "\$HEAD"}}}],
    [{"sh": "sh $root/agent.sh"}, {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "\$HEAD"}}}],
    [{"sh": "sh $root/agent.sh"}, {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "\$HEAD"}}}],
    [{"sh": "sh $root/agent.sh"}, {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "\$HEAD"}}}],
    [{"sh": "sh $root/agent.sh"}, {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "\$HEAD"}}}],
    [{"sh": "sh $root/agent.sh"}, {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "\$HEAD"}}}],
    [{"sh": "sh $root/agent.sh"}, {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "\$HEAD"}}}],
    [{"sh": "sh $root/agent.sh"}, {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "\$HEAD"}}}],
    [{"sh": "sh $root/agent.sh"}, {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "\$HEAD"}}}]
  ]
}}
JSON
for p in alpha:main beta:beta; do
  cat > "$root/c/projects/${p%%:*}.toml" <<TOML
path = "$root/repo"
base = "${p##*:}"
remote = "origin"
test = "false"
claude_bin = "$PWD/$bin/fake-claude"
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML
done

# The suite is a TAP script whose outcome the task's mode and marker files decide.
checks=$(cat <<SH
#!/bin/sh
n=\$(basename "\$PWD"); p=\$(basename "\$(dirname "\$PWD")")
now() { perl -MTime::HiRes=time -e 'printf "%d\\n", time*1000'; }
echo "\$(now) start \$p/\$n" >> "$root/compile.log"
sleep 1
mode=\$(cat "$root/b/\$n" 2>/dev/null || echo ok)
end() { echo "\$(now) end \$p/\$n" >> "$root/compile.log"; }
if [ -f skip.flag ]; then echo "1..0"; end; exit 0; fi
if [ "\$mode" = flaky ] && [ ! -f "$root/b/flake-\$n" ]; then
  touch "$root/b/flake-\$n"
  echo "1..4"; echo "ok 1 - alpha"; echo "not ok 2 - flaky_one"; echo "ok 3 - legacy"; echo "ok 4 - mine"
  end; exit 1
fi
echo "1..4"
echo "ok 1 - alpha"
if [ -f hello.txt ]; then echo "ok 2 - hello"; else echo "not ok 2 - hello"; fi
if [ -f legacy.broken ]; then echo "not ok 3 - legacy"; else echo "ok 3 - legacy"; fi
echo "ok 4 - mine"
end
if [ -f legacy.broken ] || [ ! -f hello.txt ]; then exit 1; fi
exit 0
SH
)
definition() {  # alpha explores first; beta does not
  local start=$1 first=$2
  cat <<TOML
schema = 1
start = "$start"

[resource.compile]
count = 1

$first
[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
suites = ["checks"]
on_red = "develop"
max_rounds = 4
next = "land"

[step.land]
type = "landing"
category = "landing"

[suite.checks]
command = "sh checks.sh"
parser = "tap"
min_tests = 3
level = "unit"
uses = { compile = 1 }

[suite.slow]
command = "sh slow.sh"
parser = "tap"
min_tests = 1
level = "unit"
TOML
}
alpha_first='[step.explore]
type = "agent"
category = "exploring"
next = "develop"
'
slow='#!/bin/sh
if [ -f '"$root"'/slow.flag ]; then sleep 120; fi
echo 1..1; echo "ok 1 - slow"
'
for b in main beta; do
  if [ $b = main ]; then def=$(definition explore "$alpha_first"); else def=$(definition develop ""); fi
  echo "$def" | pusher $b .mason/mason.toml "$b definition"
  echo "$checks" | pusher $b checks.sh "the suite"
  echo "$slow" | pusher $b slow.sh "a slow suite"
done

"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --max-concurrent 2 --env-retry-ms 300 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
m doctor | grep -E 'mason.toml|FAIL' || true

echo "== a parser can be checked by hand against a real tool's output"
m test parse tap /dev/stdin <<'TAP' | tail -2 || true
1..2
ok 1 - one
not ok 2 - two
TAP

echo "== 1. two projects at once, in the correct order, sharing one compile class"
m resources limit llm:claude 0 | head -1
add alpha ok "Long graph, filed first"; a=$LAST
sleep 0.2
add beta ok "Short graph, filed second"; b=$LAST
sleep 1
m resources limit llm:claude 1 | head -1
wait_state "$a" landed 150; wait_state "$b" landed 150
sa=$(first_start "$a"); sb=$(first_start "$b")
echo "alpha #$a (4 steps) started at $sa, beta #$b (3 steps) at $sb"
[ "$sb" -lt "$sa" ] || { echo "the task nearer to landing must be admitted first" >&2; exit 1; }
awk '$2=="start"{ if (open) { print "overlap at " $0; bad=1 } open=1 } $2=="end"{ open=0 } END{ exit bad }' "$root/compile.log"
m task show "$a" | grep 'tests '
m task show "$b" | grep 'tests '
m task show "$b" | grep -q 'tests     verified: checks: green, with proof'
m resources limit llm:claude 2 | head -1

echo "== 2. a vacuous pass is caught and goes back to development"
add beta vacuous "First run proves nothing"; v=$LAST
wait_state "$v" landed 150
steps=$(steps_of "$v"); echo "task #$v ran: $steps"
[ "$steps" = "develop test develop test land" ] || { echo "expected a trip back to development" >&2; exit 1; }
grep -q "$v/.*first=1 mode=vacuous" "$root/b/agent.log" || grep -q "/$v first=1 mode=vacuous" "$root/b/agent.log"
m task show "$v" | grep -q 'tests     verified'
[ "$(m task show "$v" --json | sed -n 's/^  "failures": \([0-9]*\),$/\1/p')" = 0 ] || { echo "a red is not a failure" >&2; exit 1; }
echo "(it ran 1..0 and exited 0; that is not a pass)"

echo "== 3. a flaky failure is classified on rerun"
add beta flaky "Flaky once"; f=$LAST
wait_state "$f" landed 150
steps=$(steps_of "$f"); echo "task #$f ran: $steps"
[ "$steps" = "develop test land" ] || { echo "a flaky failure must not send a task back" >&2; exit 1; }
m task show "$f" | grep 'tests '
m task show "$f" | grep -q 'tests     unverified: checks: passed only on a rerun'

echo "== 4. a baseline failure is excused"
echo "known problem" | pusher beta legacy.broken "a test the base already fails"
add beta ok "Lands despite an old failure"; l=$LAST
wait_state "$l" landed 150
m task show "$l" | grep 'tests '
m task show "$l" | grep -q 'tests     unverified: .*checks: excused, also failing at the merge base'
echo "(the base's failures are worked out once per commit and cached)"

echo "== 5. a force skip stops a running suite, keeps the green lane, records a debt"
# A task whose definition runs checks and slow; slow takes two minutes while the flag exists.
touch "$root/slow.flag"
for b in main beta; do
  if [ $b = main ]; then def=$(definition explore "$alpha_first"); else def=$(definition develop ""); fi
  echo "$def" | sed 's/suites = \["checks"\]/suites = ["checks", "slow"]/' | pusher $b .mason/mason.toml "run the slow suite too"
done
m definition reload --project beta | head -1
# The old failure is fixed on the base, so only the slow suite is in play.
git -C "$root/pusher" fetch -q origin
git -C "$root/pusher" checkout -q -B beta origin/beta
git -C "$root/pusher" rm -q -f legacy.broken
git -C "$root/pusher" -c user.name=t -c user.email=t@t commit -qm "remove the old failure"
git -C "$root/pusher" push -q origin HEAD:beta
add beta ok "Skip the slow one"; s=$LAST
until ls "$root"/d/h 2>/dev/null | grep -q -- "-u-slow$"; do sleep 0.2; done
echo "the slow suite is running"
t0=$SECONDS
m task skip "$s" --suite slow --reason "not worth two minutes" --deferred
rm -f "$root/slow.flag"
wait_state "$s" landed 60
took=$((SECONDS - t0))
echo "the running suite was stopped and the task landed ${took}s after the skip"
[ "$took" -lt 40 ]
m task show "$s" | grep -E 'tests |debt'
m task show "$s" | grep -q 'tests     unverified: checks: green, with proof; slow: skipped by force'
m debt list --all | grep -q "task #$s .*suite:slow"
# The deferred run, after landing, pays it.
end=$((SECONDS + 90))
until m debt list --all | grep -q "suite:slow *paid"; do
  [ $SECONDS -lt $end ] || { m debt list --all >&2; echo "the deferred run never paid the debt" >&2; exit 1; }
  sleep 0.5
done
m debt list --all
echo "(a debt shows until it is paid)"

echo "== quarantine always has an owner and an expiry"
if m quarantine add checks flaky_one --days 7 2>/dev/null; then echo "a quarantine without an owner was accepted" >&2; exit 1; fi
if m quarantine add checks flaky_one --owner brian --days 400 2>/dev/null; then echo "an endless quarantine was accepted" >&2; exit 1; fi
m quarantine add checks flaky_one --owner brian --days 7 --reason "flaky on CI"
m quarantine list | grep -q 'owner brian'
m quarantine remove checks flaky_one | head -1

# The rest: the parsers (a fixture for each, with a vacuous pass for each), the
# verdict and classification rules, the gates, the definition's testing tables,
# and the daemon's testing tests.
trap - EXIT; cleanup
if [ -z "${MASON_TESTS_ALREADY_RAN:-}" ]; then
  cargo test -p mason-testing
  cargo test -p mason-definition --test testing_tables
  cargo test -p mason-daemon --test testing
fi
echo "s09 OK"
