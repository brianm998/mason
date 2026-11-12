#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Limits (s48) exit test: only the machine is limited. A real masond with the scripted agent.
#  1. A non-owner flood (outside issues the owner labeled, from a fake GitHub on loopback) still
#     hits max_open: 5 issues, 3 imported, while the owner's own tasks are not counted.
#  2. The owner files 12 tasks past max_open = 3 and all are accepted; at most the llm:claude
#     limit (4 here) agent sessions run at once, and the rest wait their turn until all land.
#  3. With one compile running and the CPU source above the threshold (MASON_CPU_FILE = 85), a
#     further compile waits with the reason "CPU 85% > 50%", `mason resources` shows the reading,
#     and it runs when the file reads 20.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
command -v python3 >/dev/null || { echo "s48 needs python3" >&2; exit 1; }
root=$(mktemp -d /tmp/mason-s48.XXXXXX); daemon=""; gh=""; cc=""; first=""
cleanup() {
  [ -n "$cc" ] && kill "$cc" 2>/dev/null || true
  [ -n "$first" ] && { touch "$root/release"; kill "$first" 2>/dev/null; } || true
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  [ -n "$gh" ] && kill -9 "$gh" 2>/dev/null || true
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
fail() { set +e; echo "FAILED: $*" >&2; m task list >&2 2>&1; m resources >&2 2>&1; tail -20 "$root/masond.log" >&2; exit 1; }
count_json() { python3 -c "import json,sys; print(sum(1 for t in json.load(sys.stdin)['tasks'] if sys.argv[1] in ('', t.get('state'))))" "$1"; }
tasks() { m task list --json | count_json ""; }
landed() { m task list --json | count_json landed; }
llm_used() { m resources --json | python3 -c "import json,sys; r=[x for x in json.load(sys.stdin)['resources'] if x['name']=='llm:claude']; print(r[0]['used'] if r else 0)"; }
issue() {  # NUMBER AUTHOR LABEL-JSON
  printf ' {"number": %s, "node_id": "I_%s", "title": "Issue %s", "body": "text", "state": "open", "user": {"login": "%s"}, "labels": %s}' "$1" "$1" "$1" "$2" "$3"
}
M='[{"name": "mason"}]'

echo "== setup: a repository, the scripted agent, a fake GitHub, MASON_CPU_FILE = 5"
mkdir -p "$root/gh" "$root/c/projects"
echo '[]' > "$root/gh/issues.json"; echo '{}' > "$root/gh/deps.json"
"$bin/fake-github" "$root/gh" "$root/gh.port" 2>"$root/gh.log" &
gh=$!
until [ -s "$root/gh.port" ]; do sleep 0.05; done
port=$(cat "$root/gh.port")
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
echo base > "$root/repo/base.txt"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm init
git -C "$root/repo" push -q origin main
git -C "$root/repo" checkout -q -b owner-work
# Each agent session takes a few seconds, so sessions overlap.
cat > "$root/script.json" <<'JSON'
{"steps": {"develop": [[{"sh": "sleep 3; f=$(mktemp ./f-XXXXXX) && echo x > $f && git add -A && git -c user.name=t -c user.email=t@t commit -qm change"}, {"call": {"tool": "submit_change", "args": {"summary": "done", "head": "$HEAD"}}}]]}}
JSON
cat > "$root/c/projects/mason.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"
claude_bin = "$PWD/$bin/fake-claude"
[self]
max_open = 3
per_week = 100
max_steps = 1000
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
[tracker]
kind = "github"
api = "http://127.0.0.1:$port"
repo = "o/r"
token = "env:FAKE_GITHUB_TOKEN"
interval_s = 300
import_as = "queued"
TOML
export FAKE_GITHUB_TOKEN=recorded-test-token
echo 5 > "$root/cpu"
MASON_CPU_FILE="$root/cpu" "$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --max-concurrent 4 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
m pause --mode claims --for 1h >/dev/null

echo "== 1. a non-owner flood still hits max_open (and the owner's tasks are not counted)"
for n in 1 2 3 4 5 6 7 8 9 10 11 12; do
  m task add "owner task $n" --project mason --explored --body "x" >/dev/null || fail "the owner's task $n was refused"
done
[ "$(tasks)" = 12 ] || fail "expected 12 owner tasks, got $(tasks)"
echo "   12 owner tasks filed, all accepted (max_open is 3)"
printf '[%s,%s,%s,%s,%s]\n' "$(issue 201 stranger "$M")" "$(issue 202 stranger "$M")" "$(issue 203 stranger "$M")" "$(issue 204 stranger "$M")" "$(issue 205 stranger "$M")" > "$root/gh/issues.json"
m task refresh 0 >/dev/null; sleep 1
[ "$(tasks)" = 15 ] || fail "expected 12 owner tasks and 3 imported outside issues, got $(tasks): $(m self)"
m self | grep '^mason:' | sed 's/^/   /'
m self | grep -q '3/3 open (12 of yours open, not limited)' || fail "mason self does not show 3/3 open and 12 of yours"
for id in 12 13 14; do m task cancel "$id" >/dev/null || fail "cancel $id"; done

echo "== 2. at most 4 agent sessions at once; none refused; all 12 land"
m resume --mode claims >/dev/null
peak=0; end=$((SECONDS + 240))
while [ "$(landed)" -lt 12 ]; do
  [ $SECONDS -lt $end ] || fail "timed out: $(landed) of 12 landed"
  u=$(llm_used); [ "$u" -le 4 ] || fail "$u sessions at once, the limit is 4"
  [ "$u" -le "$peak" ] || peak=$u
  sleep 0.2
done
[ "$peak" -ge 2 ] || fail "sessions never overlapped (peak $peak)"
echo "   all 12 landed; at most $peak sessions at once (limit 4)"

echo "== 3. a further compile waits while CPU is above the threshold and runs when it falls"
# The first compile always starts (the CPU gate holds only further ones), so one holds a slot first.
m resources limit compile 2 >/dev/null
m run --needs compile -- sh -c "until [ -e '$root/release' ]; do sleep 0.1; done" 2>"$root/first.err" &
first=$!
end=$((SECONDS + 20))
until m resources | grep -q 'compile .* 1/2 in use'; do [ $SECONDS -lt $end ] || fail "the first compile did not start"; sleep 0.2; done
echo 85 > "$root/cpu"
sleep 1.5
m run --needs compile -- touch "$root/compiled" 2>"$root/cc.err" &
cc=$!
sleep 2
[ ! -e "$root/compiled" ] || fail "the compile ran at CPU 85%"
grep -q 'compile waiting: CPU 85% > 50%' "$root/cc.err" || fail "the wait was not explained: $(cat "$root/cc.err")"
m resources | grep -q 'cpu 85% busy, a compile starts below 50%' || fail "mason resources does not show the CPU: $(m resources)"
echo "   waiting: $(tail -1 "$root/cc.err")"
echo 20 > "$root/cpu"
end=$((SECONDS + 20))
until [ -e "$root/compiled" ]; do [ $SECONDS -lt $end ] || fail "the compile did not run after CPU fell"; sleep 0.2; done
wait "$cc"; cc=""
touch "$root/release"; wait "$first"; first=""
echo "   CPU fell to 20%: the compile ran"

trap - EXIT; cleanup
echo "s48 OK"
