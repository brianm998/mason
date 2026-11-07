#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Monitor answers (s40), against a real daemon (not --demo): the monitors answer questions and decide
# approvals the way their buttons do, through the monitor-tool of each client.
#  1. Four tasks stop to ask a question; a non-owner connection gets the card and no
#     buttons, and what each monitor shows matches the daemon's views (conformance).
#  2. SwiftUI clicks option 1: the task goes on and lands.
#  3. A stale card hash is refused (409); Compose sends Other text: the task goes on
#     with "Other: ..." as its answer, and lands.
#  4. Two tasks change a gate file and stop for approval; the approval view shows the
#     touched file and the diff hash; Approve with another diff hash is refused (409);
#     Approve (SwiftUI) lands the change; Reject (Compose) with a reason cancels the other.
# The agent is the scripted stand-in, so no model is used. Needs macOS and Swift; the Compose
# half runs when gradle is installed.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
[ "$(uname)" = Darwin ] && command -v swift >/dev/null || { echo "s40-real skipped: needs macOS and Swift"; exit 0; }
root=$(mktemp -d /tmp/mason-s40r.XXXXXX); daemon=""
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
(cd clients/swift && swift build 2>&1 | tail -1)
sw=clients/swift/.build/debug/monitor-tool
kt=""
if command -v gradle >/dev/null; then
  (cd clients/compose && gradle --no-daemon -q :app:installDist 2>&1 | grep -vE '^$' | head -5 || true)
  [ -x clients/compose/app/build/install/app/bin/app ] && kt=clients/compose/app/build/install/app/bin/app
fi
[ -n "$kt" ] || { echo "(Compose is not built here: the Swift tool does every step)"; kt=$sw; }
bin=target/debug
owner=${MASON_AS:-${USER:-owner}}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
jq_state() { sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
wait_state() {  # TASK STATE SECONDS
  local end=$((SECONDS + $3))
  until [ "$(m task show "$1" --json 2>/dev/null | jq_state)" = "$2" ]; do
    [ $SECONDS -lt $end ] || { echo "timed out waiting for task $1 to be $2" >&2; m task show "$1" >&2 || true; exit 1; }
    sleep 0.3
  done
}
wait_held() {
  local end=$((SECONDS + $2))
  until m task show "$1" 2>/dev/null | grep -q 'waiting for your approval'; do
    [ $SECONDS -lt $end ] || { echo "task $1 never waited for approval" >&2; m task show "$1" >&2 || true; exit 1; }
    sleep 0.3
  done
}
port() { sed -n 's/.*"port": *\([0-9]*\).*/\1/p' "$root/d/daemon.json"; }

git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
mkdir -p "$root/repo/crates/daemon/src"
echo scratch > "$root/repo/README"
echo '// the rules' > "$root/repo/crates/daemon/src/protect.rs"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm init
git -C "$root/repo" push -q origin main
git -C "$root/repo" checkout -q -b owner-work
mkdir -p "$root/c/projects"
# Every task asks first; then tasks 2 and 3 change a gate file, the others make a harmless change.
change='{"sh": "case $(basename \"$PWD\") in 2|3) mkdir -p crates/testing/src; echo weaker-$(basename \"$PWD\") > crates/testing/src/verdict.rs;; esac; echo hello > hello.txt && echo $PWD > unique-$(basename \"$PWD\").txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm work"}, {"call": {"tool": "submit_change", "args": {"summary": "did it", "head": "$HEAD"}}}'
cat > "$root/script.json" <<JSON
{"steps": {"develop": [
  [{"call": {"tool": "ask", "args": {
    "ask": "Should the token expire after one hour or one day?",
    "why": "The ticket does not say and the security notes are silent on it.",
    "options": [{"label": "One hour, with refresh tokens", "recommended": true}, {"label": "One day, without refresh tokens"}],
    "evidence": ["the ticket only says 'tokens expire'"]}}}],
  [$change], [$change]]}}
JSON
cat > "$root/c/projects/scratch.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "test -f hello.txt"
claude_bin = "$PWD/$bin/fake-claude"
[self]
max_open = 5
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --max-concurrent 4 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

for n in "token lifetime" "token lifetime, again" "gate change A" "gate change B"; do
  m task add "$n" --explored --body "Choose and implement a token lifetime." >/dev/null
done
for t in 0 1 2 3; do wait_state $t needs_attention 60; done
echo "== 1. four tasks are waiting for the owner"
m status | head -3

echo "-- a non-owner connection sees the card and no buttons"
out=$(MASON_AS=mallory "$sw" answer --data-dir "$root/d" --task 0); echo "$out"; echo "$out" | grep -q "not the owner"
[ "$(m task show 0 --json | jq_state)" = needs_attention ]
if [ "$kt" != "$sw" ]; then
  out=$(MASON_AS=mallory "$kt" answer --data-dir "$root/d" --task 0); echo "$out"; echo "$out" | grep -q "not the owner"
fi
for who in "$owner" mallory; do
  "$bin/conformance" expected --port "$(port)" --now --actor "$who" --out "$root/exp-$who" >/dev/null
  for tool in "$sw" "$kt"; do
    rm -rf "$root/act"; MASON_AS=$who "$tool" dump --data-dir "$root/d" --out "$root/act" >/dev/null
    "$bin/conformance" check --expected "$root/exp-$who" --actual "$root/act"
  done
done
echo "(both monitors show the cards as the daemon says, with buttons for $owner and none for mallory)"

echo "== 2. SwiftUI clicks option 1"
"$sw" answer --data-dir "$root/d" --task 0 --option 1
wait_state 0 landed 90
echo "task #0 went on after the click and landed"

echo "== 3. a stale card hash is refused; then Other text"
if out=$("$sw" answer --data-dir "$root/d" --task 1 --option 1 --hash 0000000000000000 2>&1); then echo "a stale hash was accepted" >&2; exit 1; fi
echo "$out"; echo "$out" | grep -q "409"; echo "$out" | grep -qi "stale"
[ "$(m task show 1 --json | jq_state)" = needs_attention ]
out=$("$kt" answer --data-dir "$root/d" --task 1 --text "Use ninety minutes, refreshed silently"); echo "$out"
# The daemon records a text answer as "Other: ..." (what the agent's next turn is given).
echo "$out" | grep -q "the daemon recorded: Other: Use ninety minutes, refreshed silently"
wait_state 1 landed 90
echo "task #1 went on after the text answer and landed"

echo "== 4. approvals"
"$sw" answer --data-dir "$root/d" --task 2 --option 1
"$sw" answer --data-dir "$root/d" --task 3 --option 1
wait_held 2 90; wait_held 3 90
for who in "$owner" mallory; do
  "$bin/conformance" expected --port "$(port)" --now --actor "$who" --out "$root/exp-$who" >/dev/null
  grep -q '"approval:2"' "$root/exp-$who/checkpoint-0.json"
  for tool in "$sw" "$kt"; do
    rm -rf "$root/act"; MASON_AS=$who "$tool" dump --data-dir "$root/d" --out "$root/act" >/dev/null
    "$bin/conformance" check --expected "$root/exp-$who" --actual "$root/act"
  done
done
echo "(the approval view matches the daemon's in both monitors: Approve and Reject for $owner, none for mallory)"
out=$(MASON_AS=mallory "$sw" approve --data-dir "$root/d" --task 2); echo "$out"; echo "$out" | grep -q "crates/testing/src/verdict.rs"; echo "$out" | grep -q "not the owner"
if out=$("$sw" approve --data-dir "$root/d" --task 2 --diff not-the-diff-shown 2>&1); then echo "another diff was approved" >&2; exit 1; fi
echo "$out"; echo "$out" | grep -q "409"
[ "$(m task show 2 --json | jq_state)" != landed ]
"$sw" approve --data-dir "$root/d" --task 2
wait_state 2 landed 90
git -C "$root/origin.git" cat-file -e main:crates/testing/src/verdict.rs
echo "task #2 landed after Approve (the diff hash it showed)"
"$kt" reject --data-dir "$root/d" --task 3 --reason "too broad for one change"
wait_state 3 canceled 60
echo "task #3 was canceled by Reject"
m metrics | head -3
echo "s40-real OK"
