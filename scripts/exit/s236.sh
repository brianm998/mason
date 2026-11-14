#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s236 exit test: a whole-machine request does not starve behind another project's work, and the
# owner can change a task's priority while its request waits. A real masond with two scratch projects
# and `[order] max_wait_s = 6`, `priority_first = true`:
#  - alpha: a three-step route (develop, test, land); its test step asks for `exclusive`.
#  - beta: a two-step route (develop, land); its three tasks each run a stream of
#    `mason run --needs test-suite` holders (limit 1), ahead of alpha by priority.
#  1. alpha's exclusive request waits behind beta's stream, then (after max_wait_s) is granted
#     while beta still has requests waiting.
#  2. `mason task priority <beta task> urgent` moves that task's waiting request ahead of the other
#     beta requests in `mason resources --json`; a non-owner is refused.
# Waits poll the state; no fixed sleeps decide a result.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
command -v python3 >/dev/null || { echo "s236 needs python3" >&2; exit 1; }
root=$(mktemp -d /tmp/mason-s236.XXXXXX); daemon=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  pkill -9 -f "$root" 2>/dev/null || true
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
fail() { set +e; echo "FAILED: $*" >&2; m task list >&2 2>&1; m resources >&2 2>&1; tail -20 "$root/masond.log" >&2; exit 1; }
task_id() { sed -n 's/.*#\([0-9]*\).*/\1/p'; }
wait_for() {  # SECONDS DESCRIPTION COMMAND...
  local end=$((SECONDS + $1)) what=$2; shift 2
  until "$@"; do [ $SECONDS -lt $end ] || fail "timed out waiting for $what"; sleep 0.2; done
}
# A python snippet over `mason resources --json`; prints, and its exit status is the answer.
res() { m resources --json | python3 -c "import json,sys; r=json.load(sys.stdin); $1"; }

mkdir -p "$root/c/projects" "$root/d"
printf '[machine]\ntest-suite = 1\n[order]\nmax_wait_s = 6\npriority_first = true\n' > "$root/c/resources.toml"

repo() {  # NAME -> a bare origin and a clone on main with the definition from stdin
  git init -q --bare -b main "$root/$1.git"
  git clone -q "$root/$1.git" "$root/$1" 2>/dev/null
  git -C "$root/$1" checkout -q -b main
  mkdir -p "$root/$1/.mason"; cat > "$root/$1/.mason/mason.toml"
  echo base > "$root/$1/base.txt"
  git -C "$root/$1" add -A
  git -C "$root/$1" -c user.name=t -c user.email=t@t commit -qm init
  git -C "$root/$1" push -q origin main
}
project() {  # NAME TEST-COMMAND
  cat > "$root/c/projects/$1.toml" <<TOML
path = "$root/$1"
base = "main"
remote = "origin"
test = "$2"
claude_bin = "$bin/fake-claude"
step_timeout_s = 600
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/$1.json"
TOML
}

repo alpha <<'TOML'
schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
uses = { exclusive = 1 }
max_rounds = 2
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
TOML
repo beta <<'TOML'
schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "land"

[step.land]
type = "landing"
category = "landing"
TOML

# alpha's agent commits and submits at once; its test step holds the machine for 3 s.
# beta's agent runs a stream of test-suite holders (2 s each), then submits.
python3 - "$root" "$bin" <<'PY'
import json, sys
root, bin_ = sys.argv[1:]
commit = {"sh": "f=$(mktemp ./f-XXXXXX) && echo x > $f && git add -A && git -c user.name=t -c user.email=t@t commit -qm change"}
submit = {"call": {"tool": "submit_change", "args": {"summary": "done", "head": "$HEAD"}}}
run = {"sh": f"{bin_}/mason --data-dir {root}/d --config-dir {root}/c run --needs test-suite -- sleep 2"}
def w(name, turns):
    json.dump({"steps": {"develop": [turns]}}, open(f"{root}/{name}.json", "w"))
w("alpha", [commit, submit])
w("beta", [run] * 8 + [commit, submit])
PY
project alpha "sleep 3"
project beta "true"

"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --max-concurrent 6 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== beta's stream holds test-suite; alpha's task arrives after it"
b1=$(m task add "beta one" --project beta --explored --priority high --body x | task_id)
b2=$(m task add "beta two" --project beta --explored --priority high --body x | task_id)
b3=$(m task add "beta three" --project beta --explored --priority high --body x | task_id)
beta_waiting() { res "sys.exit(0 if sum(1 for w in r['waiting'] if w['pipeline'] is None and w['task'] in ($b1,$b2,$b3)) >= 2 else 1)"; }
wait_for 120 "beta's stream to queue" beta_waiting
a=$(m task add "alpha" --project alpha --explored --body x | task_id)
alpha_waiting() { res "sys.exit(0 if any(w['exclusive'] and w['task'] == $a for w in r['waiting']) else 1)"; }
wait_for 120 "alpha's exclusive request to wait" alpha_waiting
echo "   alpha #$a waits for the whole machine"

echo "== after max_wait_s it is granted, while beta still has requests waiting"
# One reading of the daemon, so both facts are of the same moment.
granted_with_beta_waiting() {
  res "
h = any(x['exclusive'] for x in r['holders'])
n = sum(1 for w in r['waiting'] if w['task'] in ($b1,$b2,$b3))
sys.exit(0 if h and n >= 1 else 1)"
}
wait_for 120 "alpha's exclusive request to be granted while beta waits" granted_with_beta_waiting
echo "   granted"

echo "== a non-owner cannot change a priority; the owner can, and the request moves"
if m task priority "$b3" urgent --as someone-else >/dev/null 2>&1; then fail "a non-owner changed a priority"; fi
# b3's request waits with another beta request ahead of it.
behind() { res "
ws = [w for w in r['waiting'] if w['task'] in ($b1,$b2,$b3)]
i = [k for k, w in enumerate(ws) if w['task'] == $b3]
sys.exit(0 if i and i[0] >= 1 and ws[i[0]]['priority'] == 1 else 1)"; }
wait_for 120 "b3's request to wait behind another beta request" behind
before=$(res "
ws = [w for w in r['waiting'] if w['task'] in ($b1,$b2,$b3)]
print([k for k, w in enumerate(ws) if w['task'] == $b3][0])")
m task priority "$b3" urgent | tee "$root/prio.out"
grep -q 're-ranked' "$root/prio.out" || fail "no re-rank was reported"
after=$(res "
ws = [w for w in r['waiting'] if w['task'] in ($b1,$b2,$b3)]
i = [k for k, w in enumerate(ws) if w['task'] == $b3]
print(i[0] if i else -1, ws[i[0]]['priority'] if i else -1)")
echo "   b3's place among beta's waiting requests: $before -> ${after%% *} (priority ${after##* })"
case "$after" in
  -1*) : ;;  # granted between the two readings, which is also ahead of the others
  *) [ "${after%% *}" -lt "$before" ] || fail "the request did not move ahead: $before -> $after"
     [ "${after##* }" = 0 ] || fail "the request's priority is not urgent: $after" ;;
esac

trap - EXIT; cleanup
echo "s236 OK"
