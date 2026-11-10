#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Warn before the stage and exit-script numbers run out (s101) exit test: a real masond with the
# scripted agent (fake-claude) and a bare origin.
#  1. A daemon whose ad-hoc range starts at 960: a filed task takes s960, and `mason alerts` shows
#     a `numbers` alert saying 39 are left (999 - 960), the highest and the ceiling.
#  2. A daemon starting well below (at 100) shows no such alert after a task takes s100.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s101.XXXXXX); daemon=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  for hd in "$root"/d/h; do target/debug/step-host-ctl reap "$hd" >/dev/null 2>&1; done
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
owner=${USER:-owner}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { echo "FAILED: $*" >&2; m alerts >&2 2>&1 || true; tail -20 "$root/masond.log" >&2 || true; exit 1; }
add() { m task add "$1" --project web --body "${2:-do it}" | sed -n 's/^Filed task #\([0-9]*\) .*/\1/p'; }
num() { m task show "$1" | sed -n "s/^  number    $2 \([0-9]*\):.*/\1/p"; }
start_daemon() {
  "$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
  daemon=$!
  until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
}
stop_daemon() { kill "$daemon" 2>/dev/null || true; wait "$daemon" 2>/dev/null || true; daemon=""; }
ci="git -c user.name=t -c user.email=t@t"

echo "== setup: a bare origin and a scripted agent"
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
mkdir -p "$root/repo/.mason"
printf '# Repo\n' > "$root/repo/README.md"
cat > "$root/repo/.mason/mason.toml" <<'TOML'
schema = 1
start = "explore"

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
max_rounds = 1
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
max_rounds = 3
on_red = "develop"
TOML
printf 'mode = "direct"\n' > "$root/repo/.mason/landing.toml"
(cd "$root/repo" && git add -A && $ci commit -qm "base" && git push -q origin main)
mkdir -p "$root/c/projects"
cat > "$root/script.json" <<JSON
{"steps": {"explore": [[
  {"call": {"tool": "submit_plan", "args": {"summary": "x", "files": ["docs"], "approach": "x", "acceptance_criteria": ["x"]}}}
]]}}
JSON
cat > "$root/c/projects/web.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"
claude_bin = "$PWD/$bin/fake-claude"
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML

take() {  # a filed task takes its number
  local t; t=$(add "$1" "x")
  local end=$((SECONDS + 30)); until [ -n "$(num "$t" exit)" ]; do [ $SECONDS -lt $end ] || fail "task #$t got no number"; sleep 0.2; done
  m pause --mode claims --for 1h >/dev/null
  echo "$t"
}

echo "== 1. range starts at 960: the first task takes s960 and the warning counts what is left"
MASON_ADHOC_EXIT_START=960 start_daemon
t=$(take "near the end")
[ "$(num "$t" exit)" = 960 ] || fail "expected s960, got $(num "$t" exit)"
end=$((SECONDS + 30)); until m alerts | grep -q 'numbers'; do [ $SECONDS -lt $end ] || fail "no numbers alert"; sleep 0.2; done
line=$(m alerts | grep 'numbers' | head -1)
echo "   $line"
echo "$line" | grep -q '39 stage and exit-script numbers left' || fail "wrong count: $line"
echo "$line" | grep -q 'highest handed out is 960' || fail "no highest: $line"
echo "$line" | grep -q 'ceiling is 999' || fail "no ceiling: $line"
echo "$line" | grep -q 'scripts/run-exit-scripts.sh' || fail "no pointer to the scripts: $line"
stop_daemon

echo "== 2. a daemon starting well below shows no warning"
rm -rf "$root/d" "$root/c/state" 2>/dev/null || true
MASON_ADHOC_EXIT_START=100 start_daemon
t=$(take "far from the end")
[ "$(num "$t" exit)" = 100 ] || fail "expected s100, got $(num "$t" exit)"
sleep 1
if m alerts | grep -q 'numbers'; then fail "a numbers alert with 899 numbers left"; fi
echo "   no numbers alert with 899 left"
echo "s101 OK"
