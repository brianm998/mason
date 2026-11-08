#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# LLM provider accounts (s56) exit test: a real masond with the scripted agent (fake-claude).
#  1. Two accounts (a, b) with one session each and two projects, one per account: a task in each
#     runs at the same time, though the daemon-wide --max-concurrent is 1 (they share no slot).
#  2. `mason resources` lists llm:a and llm:b, each limit 1; no llm:claude.
#  3. `mason doctor` lists both accounts with adapter, billing and the project that uses each.
#  4. A project that names an account that does not exist stops the daemon starting, naming it.
#  5. With no providers.toml there is one account, claude, as before: doctor and resources show it
#     and a task runs and lands.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s56.XXXXXX); daemon=""
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
owner=${USER:-owner}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { echo "FAILED: $*" >&2; m task list >&2 2>&1 || echo "(task list failed)" >&2; tail -20 "$root/masond.log" >&2 || echo "(no daemon log)" >&2; exit 1; }
state_of() { m task show "$1" --json 2>/dev/null | sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
wait_state() {  # TASK STATE SECONDS
  local end=$((SECONDS + $3))
  until [ "$(state_of "$1")" = "$2" ]; do
    [ $SECONDS -lt $end ] || { m task show "$1" >&2 || echo "(task show failed)" >&2; fail "timed out waiting for task $1 to be $2"; }
    sleep 0.2
  done
}
add() { m task add "$2" --project "$1" --body "do it" | sed -n 's/^Filed task #\([0-9]*\) .*/\1/p'; }
start_daemon() {
  "$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --max-concurrent 1 --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
  daemon=$!
  until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
}
stop_daemon() { if [ -n "$daemon" ]; then kill "$daemon" 2>/dev/null || echo "(daemon already gone)" >&2; wait "$daemon" 2>/dev/null || echo "(daemon exited nonzero)" >&2; fi; daemon=""; }
ci="git -c user.name=t -c user.email=t@t"

echo "== setup: a bare origin, a one-agent-step pipeline, and a scripted agent that works for 4 s"
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
mkdir -p "$root/repo/.mason" "$root/c/projects"
cat > "$root/repo/.mason/mason.toml" <<'TOML'
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
printf 'mode = "direct"\n' > "$root/repo/.mason/landing.toml"
(cd "$root/repo" && git add -A && $ci commit -qm base && git push -q origin main)
cat > "$root/script.json" <<'JSON'
{"steps": {"develop": [[
  {"sh": "sleep 4; f=$(basename $(dirname $PWD))-$(basename $PWD).txt; echo x > $f; git add -A; git -c user.name=t -c user.email=t@t commit -qm work"},
  {"call": {"tool": "submit_change", "args": {"summary": "done", "head": "$HEAD"}}}
]]}}
JSON
project() {  # ID EXTRA
  cat > "$root/c/projects/$1.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"
claude_bin = "$PWD/$bin/fake-claude"
$2
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML
}

echo "== 1. two accounts, one session each, one project per account"
cat > "$root/c/providers.toml" <<'TOML'
# SPDX-License-Identifier: GPL-3.0-or-later
[provider.a]
adapter = "claude"
billing = "subscription"
sessions = 1

[provider.b]
adapter = "claude"
billing = "api"
sessions = 1
monthly_budget_usd = 50
TOML
project p1 'account = "a"'
project p2 'account = "b"'
start_daemon
res=$(m resources 2>&1)
echo "$res" | sed 's/^/   /'
echo "$res" | grep -q 'llm:a' || fail "mason resources lacks llm:a"
echo "$res" | grep -q 'llm:b' || fail "mason resources lacks llm:b"
! echo "$res" | grep -q 'llm:claude' || fail "an account nobody configured appeared"
t1=$(add p1 "on account a"); t2=$(add p2 "on account b")
both=""
end=$((SECONDS + 60))
while [ $SECONDS -lt $end ]; do
  s1=$(state_of "$t1"); s2=$(state_of "$t2")
  if [ "$s1" = developing ] && [ "$s2" = developing ]; then both=yes; break; fi
  sleep 0.1
done
[ -n "$both" ] || fail "the two tasks never ran at the same time"
echo "   task #$t1 (account a) and #$t2 (account b) are developing at the same time"
wait_state "$t1" landed 90; wait_state "$t2" landed 90
echo "   both landed"

echo "== 3. doctor lists both accounts"
doc_rc=0; doc=$(m doctor 2>&1) || doc_rc=$?; echo "   doctor exit status $doc_rc"
echo "$doc" | grep 'account ' | sed 's/^/   /'
echo "$doc" | grep -q 'account a: adapter claude, billing subscription, projects p1' || fail "doctor lacks account a"
echo "$doc" | grep -q 'account b: adapter claude, billing api, projects p2' || fail "doctor lacks account b"
stop_daemon

echo "== 4. a project naming a missing account stops the daemon starting, naming it"
project p3 'account = "missing"'
set +e
out=$("$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --owner "$owner" 2>&1)
rc=$?
set -e
echo "$out" | sed 's/^/   /'
[ "$rc" -ne 0 ] || fail "the daemon started with a project on a missing account"
echo "$out" | grep -q '"missing"' || fail "the refusal does not name the account"
echo "$out" | grep -q 'p3' || fail "the refusal does not name the project"
rm "$root/c/projects/p3.toml"

echo "== 5. with no providers.toml: one account, claude, as before"
rm "$root/c/providers.toml"
project p1 ''; rm "$root/c/projects/p2.toml"
start_daemon
res=$(m resources 2>&1)
echo "$res" | grep -q 'llm:claude' || fail "resources lacks llm:claude"
! echo "$res" | grep -q 'llm:a' || fail "an old account is still listed"
doc_rc=0; doc=$(m doctor 2>&1) || doc_rc=$?; echo "   doctor exit status $doc_rc"
echo "$doc" | grep -q 'account claude: adapter claude, billing subscription, projects p1' || fail "doctor lacks the default account"
t=$(add p1 "default account")
wait_state "$t" landed 90
echo "   a task on the default account landed"
echo "s56 exit test passed"
