#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Never test the same tree twice (s179) exit test: a real masond and the fake agent. The test
# command appends a line to a marker file each time it runs. The agent calls run_check (green),
# commits and submits; the test step reuses the green, and the marker has exactly one line when
# the task lands.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s179.XXXXXX); daemon=""
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

commit='{"sh": "echo hello > hello.txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm work"}'
check='{"call": {"tool": "run_check", "args": {"name": "tests"}}}'
submit='{"call": {"tool": "submit_change", "args": {"summary": "did it", "head": "$HEAD"}}}'
echo "{\"steps\": {\"develop\": [[$commit, $check, $submit]]}}" > "$root/work.json"
mkdir -p "$root/c/projects" "$root/d"

git init -q --bare -b main "$root/o.git"
git clone -q "$root/o.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
echo scratch > "$root/repo/README"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm init
git -C "$root/repo" push -q origin main
printf 'echo run >> %s/marker\ntest -f hello.txt\n' "$root" > "$root/t.sh"
cat > "$root/c/projects/base.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "sh $root/t.sh"
claude_bin = "$PWD/$bin/fake-claude"
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/work.json"
TOML

"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --max-concurrent 3 --owner "${USER:-owner}" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== run_check is green, the test step reuses it, the command runs once"
t=$(m task add "Checked once" --project base --explored --body "x" | task_id)
wait_for 180 "the task to land" state_is "$t" landed
n=$(wc -l < "$root/marker" | tr -d ' ')
[ "$n" = 1 ] || fail "the test command ran $n times, not once"
m task show "$t" | grep -q 'green for this exact tree already' || echo "   (note not shown in task show)"
echo "   landed; the marker has one line"
echo "s179: ok"
