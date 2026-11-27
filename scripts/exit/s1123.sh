#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1123 exit test: nodes only (#1150). Two real daemons on loopback: a coordinator with a
# project `scratch` that has no `[remote_runs]`, and a node. Then:
#  1. `mason remote status` shows scratch as "always runs here";
#  2. `mason remote mode remote --project scratch` is accepted, writes `[placement]` into the
#     registration file, and status shows scratch as nodes only, not "always runs here";
#  3. with the node paired and taking work, a task of scratch lands and its suite ran on the node
#     and nowhere else.
set -euo pipefail
trap 'rc=$?; [ "$rc" -eq 0 ] || echo "FAILED (exit $rc) at line $LINENO: $BASH_COMMAND" >&2' ERR
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s1123.XXXXXX); daemon=""; node=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  [ -n "$node" ] && kill -9 "$node" 2>/dev/null || true
  for hd in "$root"/d/h "$root"/nd/h; do target/debug/step-host-ctl reap "$hd" >/dev/null 2>&1 || true; done
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=$PWD/target/debug
owner=${USER:-owner}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
mn() { "$bin/mason" --data-dir "$root/nd" --config-dir "$root/nc" "$@"; }
jf() {  # FILTER: a python expression over the parsed document `j` on stdin
  python3 -I -c 'import json,sys; j=json.load(sys.stdin); print(eval(sys.argv[1]))' "$1"
}
jq_state() { sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
fail() { echo "FAILED: $*" >&2; set +e; m task show 0 >&2 2>&1; m node list >&2 2>&1; tail -30 "$root/masond.log" >&2; tail -30 "$root/node.log" >&2; exit 1; }
export MASON_NO_MDNS=1

echo "== setup: a repository, a coordinator and a node"
mkdir -p "$root/c/projects" "$root/nc" "$root/nd"
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
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
suites = ["checks"]
on_red = "develop"
max_rounds = 2
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

TOML
# The suite records where each of its runs was.
cat > "$root/repo/checks.sh" <<SH
if [ -n "\$MASON_NODE" ]; then echo node >> "$root/ran"; else echo here >> "$root/ran"; fi
echo 1..3
echo "ok 1 - a"; echo "ok 2 - b"; echo "ok 3 - c"
SH
echo scratch > "$root/repo/README"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm "base"
git -C "$root/repo" push -q origin main
git -C "$root/repo" fetch -q origin

cat > "$root/agent.sh" <<'SH'
echo hello > "hello-$$.txt"
git add -A; git -c user.name=t -c user.email=t@t commit -qm "work"
SH
step='[{"sh": "sh '"$root"'/agent.sh"}, {"call": {"tool": "submit_change", "args": {"summary": "added hello.txt", "head": "$HEAD"}}}]'
cat > "$root/script.json" <<JSON
{"steps": {"develop": [$step, $step, $step]}}
JSON
cat > "$root/c/projects/scratch.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "false"
claude_bin = "$bin/fake-claude"

[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML
printf '[node]\nserve = true\nport = 0\nbind = ["127.0.0.1"]\n' > "$root/nc/daemon.toml"
# This test is about placement, not disk limits (unit tests cover those): the coordinator tells
# the node to ask for no free space, so the result does not depend on how full this host's disks
# are (#1246). No assertion is changed.
printf '[nodes.default]\nmin_disk_free_gb = 0\nmin_other_disk_free_gb = 0\n' > "$root/c/daemon.toml"

# The node is a development build, says it has Rust, and reads its CPU use from a file.
echo 0 > "$root/cpu"
MASON_NODE_ALLOW_UNSIGNED=1 MASON_CPU_FILE="$root/cpu" \
  MASON_NODE_TOOLCHAINS='{"rust":["1.96.0"],"xcode":[],"java":[]}' \
  "$bin/masond" --port 0 --data-dir "$root/nd" --config-dir "$root/nc" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/node.log" &
node=$!
MASON_NODE_POLL_MS=50 MASON_CPU_FILE="$root/cpu" \
  "$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
until mn node status >/dev/null 2>&1; do kill -0 "$node" || { cat "$root/node.log" >&2; exit 1; }; sleep 0.1; done

echo "== 1. no mode: it always runs here"
m remote status | grep 'scratch' | grep -q 'always runs here' || fail "scratch with no mode is not 'always runs here'"

echo "== 2. a mode of its own: nodes only"
m remote mode remote --project scratch >/dev/null || fail "the mode was refused"
grep -q '^\[placement\]' "$root/c/projects/scratch.toml" || fail "no [placement] in the registration file"
line=$(m remote status | grep '^  scratch')
case "$line" in
  *"remote (nodes only: no GitHub setup)"*) ;;
  *) fail "status does not show scratch as nodes only: $line" ;;
esac
case "$line" in *"always runs here"*) fail "still 'always runs here': $line" ;; esac
[ "$(m remote status --json | jf "[p for p in j['projects'] if p['project']=='scratch'][0]['nodes_only']")" = True ] || fail "status JSON does not say nodes_only"

echo "== 3. pair the node; the suite is placed on it"
code=$(mn node pair-code --json)
m node add --host "127.0.0.1:$(echo "$code" | jf "j['port']")" --fp "$(echo "$code" | jf "j['fingerprint']")" \
  --code "$(echo "$code" | jf "j['code']")" --name studio >/dev/null || fail "pairing"
mn node accept on >/dev/null || fail "accept on"
m node use on >/dev/null || fail "node use on"
m task add "On the node" --project scratch --body "x" >/dev/null
end=$((SECONDS + 240))
until [ "$(m task show 0 --json 2>/dev/null | jq_state)" = landed ]; do
  [ $SECONDS -lt $end ] || fail "the task did not land"
  sleep 0.2
done
grep -q '^node$' "$root/ran" || fail "the suite did not run on the node"
grep -q '^here$' "$root/ran" && fail "the suite ran on the coordinator"
echo "s1123 passed"
