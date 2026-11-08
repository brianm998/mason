#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 12, against a real daemon (not --demo): a task asks a question, the
# monitor's own answer path (the same code the decision card's buttons run)
# answers it with the card's hash, and the task resumes and lands.
# The agent is the scripted stand-in, so no model is used. Needs macOS and Swift.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
[ "$(uname)" = Darwin ] && command -v swift >/dev/null || { echo "s12-real skipped: needs macOS and Swift"; exit 0; }
root=$(mktemp -d /tmp/mason-s12r.XXXXXX); daemon=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  # Verified before it is killed: a stale pid in a registry may belong to someone else now.
  for hd in "$root"/d/h; do target/debug/step-host-ctl reap "$hd" >/dev/null 2>&1 || true; done
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
(cd clients/swift && swift build 2>&1 | tail -1)
tool=clients/swift/.build/debug/monitor-tool
bin=target/debug
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }

git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
echo scratch > "$root/repo/README"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm init
git -C "$root/repo" push -q origin main
git -C "$root/repo" checkout -q -b owner-work
mkdir -p "$root/c/projects"
turn='{"sh": "echo hello > hello.txt && echo $PWD > unique-$(basename \"$PWD\").txt && git add -A && git -c user.name=t -c user.email=t@t commit -qm work"}, {"call": {"tool": "submit_change", "args": {"summary": "did it", "head": "$HEAD"}}}'
cat > "$root/script.json" <<JSON
{"steps": {"develop": [
  [{"call": {"tool": "ask", "args": {
    "ask": "Should the token expire after one hour or one day?",
    "why": "The ticket does not say and the security notes are silent on it.",
    "options": [{"label": "One hour, with refresh tokens", "recommended": true}, {"label": "One day, without refresh tokens"}],
    "evidence": ["the ticket only says 'tokens expire'"]}}}],
  [$turn], [$turn]]}}
JSON
cat > "$root/c/projects/scratch.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "test -f hello.txt"
claude_bin = "$PWD/$bin/fake-claude"
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

m task add "Token lifetime" --explored --body "Choose and implement a token lifetime." >/dev/null
end=$((SECONDS + 60))
until m task show 0 --json | grep -q '"state": "needs_attention"'; do
  [ $SECONDS -lt $end ] || { m task show 0 >&2; echo "the task never asked" >&2; exit 1; }; sleep 0.2
done
echo "task #0 is waiting for the owner"
"$tool" answer --data-dir "$root/d" --task 0 --option 1
end=$((SECONDS + 90))
until m task show 0 --json | grep -q '"state": "landed"'; do
  [ $SECONDS -lt $end ] || { m task show 0 >&2; echo "the answered task did not land" >&2; exit 1; }; sleep 0.3
done
echo "task #0 resumed after the monitor's answer and landed"
m task show 0 | grep -E 'state|failures'
m metrics | head -3
echo "s12-real OK"
