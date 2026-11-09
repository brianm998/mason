#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s75 exit test: releasing, adding and reviving blockers (owner only). A real masond, a fake
# GitHub, command steps standing in for the agent (no model is called).
#  1. A task blocked by a cancelled task is shown held, with the commands, in `task show` and
#     the alert; `mason task unblock` releases it, the alert clears, and it runs to landed.
#  2. A task added with --after gets a second blocker with `mason task block`; one release keeps
#     it held; a cycle is refused naming it; `retry` on a cancelled task is refused and
#     `mason task revive` puts it back in the queue.
#  3. On a tracker-imported task the release survives two refresh polls, and lapses when the
#     fake host changes the relationship.
#  4. A non-owner is refused by unblock, block and revive.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s75.XXXXXX); daemon=""; gh=""
cleanup() {
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
fail() { echo "FAILED: $*" >&2; set +e; m task list >&2 2>&1; m alerts >&2 2>&1; tail -20 "$root/masond.log" >&2; exit 1; }
issue() {  # NUMBER
  printf ' {"number": %s, "node_id": "I_%s", "title": "Issue %s", "body": "text", "state": "open", "user": {"login": "%s"}, "labels": []}' "$1" "$1" "$1" "$owner"
}
state_of() { m task show "$1" --json 2>/dev/null | sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
wait_state() { local end=$((SECONDS + $3)); until [ "$(state_of "$1")" = "$2" ]; do [ $SECONDS -lt $end ] || fail "timed out waiting for task $1 to be $2 (is $(state_of "$1"))"; sleep 0.2; done; }
id_of() { m task list | grep "$1" | head -1 | sed -E 's/^#?([0-9]+).*/\1/'; }
new_id() { sed -E 's/^Filed task #([0-9]+).*/\1/'; }
blockers_of() { m task show "$1" --json | python3 -I -c 'import json,sys; print(",".join(str(n) for n in sorted(json.load(sys.stdin).get("blocked_by") or [])))'; }
show_has() { m task show "$1" | grep -q -- "$2" || { m task show "$1" >&2; fail "task show $1 does not say: $2"; }; }
alerts_have() { m alerts | grep -q -- "$1" || { m alerts >&2; fail "no alert saying: $1"; }; }
alerts_lack() { ! m alerts | grep -q -- "$1" || { m alerts >&2; fail "an alert still says: $1"; }; }
refuses() {  # WHAT COMMAND...
  local what=$1; shift
  if out=$("$@" 2>&1); then fail "$what was accepted: $out"; fi
  echo "$out"
}

echo "== setup: a repository, a project with a tracker, a fake GitHub"
mkdir -p "$root/gh" "$root/c/projects"
echo '[]' > "$root/gh/issues.json"; echo '{}' > "$root/gh/deps.json"
"$bin/fake-github" "$root/gh" "$root/gh.port" 2>"$root/gh.log" & gh=$!
until [ -s "$root/gh.port" ]; do sleep 0.05; done
port=$(cat "$root/gh.port")
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
mkdir -p "$root/repo/.mason"
cat > "$root/repo/.mason/mason.toml" <<'TOML'
schema = 1
start = "develop"

[step.develop]
type = "command"
category = "developing"
command = "f=$(mktemp ./f-XXXXXX) && echo work > $f && git add -A && git -c user.name=t -c user.email=t@t commit -qm work"
next = "test"

[step.test]
type = "test"
category = "testing"
max_rounds = 1
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
TOML
printf 'mode = "direct"\n' > "$root/repo/.mason/landing.toml"
echo base > "$root/repo/base.txt"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm "pipeline"
git -C "$root/repo" push -q origin main
cat > "$root/c/projects/self.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"

[tracker]
kind = "github"
api = "http://127.0.0.1:$port"
repo = "o/r"
token = "env:FAKE_GITHUB_TOKEN"
interval_s = 300
TOML
export FAKE_GITHUB_TOKEN=recorded-test-token
# A free port, not the default one a real daemon may hold.
dport=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
"$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --port "$dport" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== 1. a task blocked by a cancelled task is held, with the commands; unblock frees it and it runs"
x=$(m task add "blocker X" --project self --explored --proposed | new_id)
y=$(m task add "dependent Y" --project self --explored --after "$x" | new_id)
m task cancel "$x" >/dev/null
wait_state "$x" canceled 30
show_has "$y" "held: blocked by #$x (cancelled)"
show_has "$y" "mason task unblock $y --from $x"
show_has "$y" "mason task revive $x"
show_has "$y" "set by you"
alerts_have "mason task unblock $y --from $x"
[ "$(m alerts | grep -c "Task #$y is held")" = 1 ] || { m alerts >&2; fail "the alert for #$y is not said exactly once"; }
[ "$(state_of "$y")" = queued ] || fail "Y is $(state_of "$y"), not queued"
m task unblock "$y" --from "$x" | grep -q "released task #$y" || fail "unblock did not say what changed"
alerts_lack "Task #$y is held"
wait_state "$y" landed 120
echo "   Y shown held with the commands, released, and landed"

echo "== 2. block adds a second blocker; a release keeps it held; a cycle is refused; revive"
p1=$(m task add "P1" --project self --explored --proposed | new_id)
p2=$(m task add "P2" --project self --explored --proposed | new_id)
t=$(m task add "T after P1" --project self --explored --proposed --after "$p1" | new_id)
[ "$(blockers_of "$t")" = "$p1" ] || fail "T is blocked by $(blockers_of "$t")"
m task block "$t" --after "$p2" | grep -q "now waits for #$p2" || fail "block did not say what changed"
[ "$(blockers_of "$t")" = "$p1,$p2" ] || [ "$(blockers_of "$t")" = "$p2,$p1" ] || fail "T is blocked by $(blockers_of "$t"), not both"
m task unblock "$t" --from "$p1" | grep -q "still held by #$p2" || fail "release of one of two did not say it is still held"
[ "$(blockers_of "$t")" = "$p2" ] || fail "T is blocked by $(blockers_of "$t"), not just P2"
out=$(refuses "a cycle" m task block "$p2" --after "$t")
echo "$out" | grep -q "cycle" && echo "$out" | grep -q "#$t" && echo "$out" | grep -q "#$p2" || fail "the cycle is not named: $out"
refuses "releasing a task that is not a blocker" m task unblock "$t" --from "$p1" >/dev/null
m task cancel "$p2" >/dev/null
wait_state "$p2" canceled 30
refuses "retry of a cancelled task" m task retry "$p2" >/dev/null
alerts_have "mason task revive $p2"
m task revive "$p2" | grep -q "queued again" || fail "revive did not say what changed"
[ "$(state_of "$p2")" != canceled ] || fail "P2 is still cancelled after revive"
alerts_lack "Task #$t is held"
m task cancel "$p2" >/dev/null
wait_state "$p2" canceled 30
echo "   second blocker added, one released, cycle refused naming it, cancelled blocker revived"

echo "== 3. a tracker-imported task: the release survives two polls and lapses when GitHub changes"
G=$root/gh
printf '[%s,%s]\n' "$(issue 601)" "$(issue 602)" > "$G/issues.json"
echo '{"602": [601]}' > "$G/deps.json"
m task refresh 0 >/dev/null; sleep 1
t601=$(id_of "Issue 601"); t602=$(id_of "Issue 602")
[ -n "$t601" ] && [ -n "$t602" ] || fail "the issues were not imported"
[ "$(blockers_of "$t602")" = "$t601" ] || fail "602's task is blocked by $(blockers_of "$t602"), not $t601"
show_has "$t602" "from GitHub"
m task unblock "$t602" --all >/dev/null
[ -z "$(blockers_of "$t602")" ] || fail "the release did not hold"
for i in 1 2; do
  m task refresh 0 >/dev/null; sleep 1
  [ -z "$(blockers_of "$t602")" ] || fail "poll $i put the released blocker back"
done
echo '{"602": []}' > "$G/deps.json"
m task refresh 0 >/dev/null; sleep 1
[ -z "$(blockers_of "$t602")" ] || fail "an empty relationship held the task"
echo '{"602": [601]}' > "$G/deps.json"
m task refresh 0 >/dev/null; sleep 1
[ "$(blockers_of "$t602")" = "$t601" ] || fail "the release did not lapse: blocked by '$(blockers_of "$t602")'"
show_has "$t602" "from GitHub"
echo "   released, kept through two polls, back after GitHub changed the relationship"

echo "== 4. a non-owner is refused"
refuses "a non-owner unblock" env MASON_AS=intruder "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" task unblock "$t602" --all | grep -qi "owner" || fail "the refusal does not say only the owner can"
refuses "a non-owner block" env MASON_AS=intruder "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" task block "$t602" --after "$p1" >/dev/null
refuses "a non-owner revive" env MASON_AS=intruder "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" task revive "$x" >/dev/null
[ "$(blockers_of "$t602")" = "$t601" ] || fail "a refused command changed the blockers"
echo "   unblock, block and revive refused for a non-owner; nothing changed"

trap - EXIT; cleanup
echo "s75 OK"
