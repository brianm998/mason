#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Test reruns run only the failed tests (s165) exit test: a real masond, the fake agent, and a
# project whose test command is a script that prints libtest lines and reads its filter arguments
# (`test_reruns = 1`, `rerun_filter = "sh t.sh {tests}"`).
#  1. Run 1 fails `alpha`; the rerun receives only `alpha` and passes. The task lands, and
#     `mason health` lists `alpha` as flaky.
#  2. `alpha` fails every time. The task is sent back and the feedback names `alpha`.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s165.XXXXXX); daemon=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  for hd in "$root"/d/h; do target/debug/step-host-ctl reap "$hd" >/dev/null 2>&1; done
  rm -rf "$root"
}
trap cleanup EXIT

cargo build -q --workspace --bins
bin=target/debug
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { set +e; echo "FAILED: $*" >&2; tail -20 "$root/masond.log" >&2; exit 1; }
jq_state() { sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
task_id() { sed -n 's/.*#\([0-9]*\).*/\1/p'; }
wait_for() {  # SECONDS DESCRIPTION COMMAND...
  local end=$((SECONDS + $1)) what=$2; shift 2
  until "$@"; do [ $SECONDS -lt $end ] || fail "timed out waiting for $what"; sleep 0.2; done
}
state_is() { [ "$(m task show "$1" --json 2>/dev/null | jq_state)" = "$2" ]; }
has_feedback() { grep -q "$2" "$root/d/t/$1/feedback.txt" 2>/dev/null; }

turn='{"sh": "echo hello > hello.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm work"}, {"call": {"tool": "submit_change", "args": {"summary": "did it", "head": "$HEAD"}}}'
echo "{\"steps\": {\"develop\": [[$turn], [$turn]]}}" > "$root/work.json"
mkdir -p "$root/c/projects" "$root/d"

project() {  # NAME TEST-SCRIPT-BODY: a repo, its origin and a project whose test is that script
  local name=$1
  git init -q --bare -b main "$root/$name.git"
  git clone -q "$root/$name.git" "$root/$name" 2>/dev/null
  git -C "$root/$name" checkout -q -b main
  echo scratch > "$root/$name/README"
  git -C "$root/$name" add -A
  git -C "$root/$name" -c user.name=t -c user.email=t@t commit -qm init
  git -C "$root/$name" push -q origin main
  # Counts its runs, records the arguments it was given (one line per run), then the body.
  printf 'n=$(cat %s/%s.runs 2>/dev/null || echo 0); echo $((n+1)) > %s/%s.runs\necho "$*" >> %s/%s.args\n%s\n' \
    "$root" "$name" "$root" "$name" "$root" "$name" "$2" > "$root/$name.sh"
  cat > "$root/c/projects/$name.toml" <<TOML
path = "$root/$name"
base = "main"
remote = "origin"
test = "sh $root/$name.sh"
test_reruns = 1
rerun_filter = "sh $root/$name.sh {tests}"
claude_bin = "$PWD/$bin/fake-claude"
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/work.json"
TOML
}
project flaky 'if [ $# -eq 0 ]; then echo "test alpha ... FAILED"; echo "test beta ... ok"; exit 1; fi; echo "test alpha ... ok"'
# Fails only where the change is (hello.txt): at the merge base it passes, so s177's base check
# excuses nothing.
project broken '[ -f hello.txt ] || exit 0; echo "test alpha ... FAILED"; exit 1'

"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --max-concurrent 3 --owner "${USER:-owner}" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== 1. run 1 fails alpha; the rerun is given only alpha and passes"
t=$(m task add "Flaky once" --project flaky --explored --body "x" | task_id)
wait_for 180 "the task to land" state_is "$t" landed
[ "$(cat "$root/flaky.args")" = "$(printf '\nalpha')" ] || fail "the rerun's arguments were: $(cat "$root/flaky.args")"
[ "$(cat "$root/flaky.runs")" = 2 ] || fail "the command ran $(cat "$root/flaky.runs") times"
m health | grep -q 'flaky::alpha' || fail "mason health does not list alpha as flaky"
m health | grep -q 'flaky::beta' && fail "beta did not fail and must not be listed"
m health | grep 'alpha' | sed 's/^/   /'
echo "   landed; alpha is flaky"

echo "== 2. alpha fails every time: the task is sent back, and the feedback names alpha"
t=$(m task add "Broken" --project broken --explored --body "x" | task_id)
wait_for 180 "the feedback" has_feedback "$t" 'failed every time: alpha'
st=$(m task show "$t" --json | jq_state)
[ "$st" != landed ] || fail "a red task landed"
grep -q beta "$root/d/t/$t/feedback.txt" && fail "the feedback names beta, which passed"
[ "$(head -2 "$root/broken.args")" = "$(printf '\nalpha')" ] || fail "the rerun's arguments were: $(cat "$root/broken.args")"
echo "   sent back (now $st) naming alpha"
echo "s165: ok"
