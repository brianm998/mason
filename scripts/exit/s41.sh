#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 41 exit test: the self-hold scan holds only what a task really weakens, and says
# why in one short line, through the CLI of a real masond.
#  1. A task adding a new exit script full of cleanup lines (kill/rm/wait/trap ... || true)
#     is not held, and lands.
#  2. A task adding a new exit script AND wiring it into scripts/run-exit-scripts.sh is held
#     on the runner change only, naming file and line.
#  3. A task adding a real `cargo test || true` to an existing gate is held, naming file and line.
#  4. A task that merges in a base which landed a protected change is not held for it.
#  5. A task touching only clients/swift and docs is never held.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s41.XXXXXX); daemon=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  for f in "$root"/d/h/*/host.kv; do
    [ -e "$f" ] || continue
    hp=$(sed -n "s/^host_pid=//p" "$f"); cp=$(sed -n "s/^child_pid=//p" "$f"); if [ -n "$hp" ] && ps -o command= -p "$hp" 2>/dev/null | grep -qF "step-host --dir $(dirname "$f")"; then kill -9 "-$cp" 2>/dev/null || true; kill -9 "$hp" 2>/dev/null || true; fi  # only a live step host: a stale pid may belong to someone else now
  done
  rm -rf "$root"
}
trap cleanup EXIT

cargo build -q --workspace --bins
bin=target/debug
owner=${USER:-owner}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
jq_state() { sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
wait_state() {  # TASK STATE SECONDS
  local end=$((SECONDS + $3))
  until [ "$(m task show "$1" --json 2>/dev/null | jq_state)" = "$2" ]; do
    [ $SECONDS -lt $end ] || { echo "timed out waiting for task $1 to be $2" >&2; m task show "$1" >&2 || true; exit 1; }
    sleep 0.3
  done
}
wait_held() {  # TASK SECONDS: the task shows that it waits for the owner
  local end=$((SECONDS + $2))
  until m task show "$1" 2>/dev/null | grep -q 'waiting for your approval'; do
    [ $SECONDS -lt $end ] || { echo "task $1 never waited for approval" >&2; m task show "$1" >&2 || true; exit 1; }
    sleep 0.3
  done
}
on_origin() { git -C "$root/origin.git" cat-file -e "main:$1" 2>/dev/null; }

git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
mkdir -p "$root/repo/scripts/exit" "$root/repo/docs" "$root/repo/clients/swift"
printf '#!/usr/bin/env bash\necho gate\n' > "$root/repo/scripts/exit/s17.sh"
printf '#!/usr/bin/env bash\necho gate\n' > "$root/repo/scripts/exit/s16.sh"
printf '#!/usr/bin/env bash\nfor s in scripts/exit/s1*.sh; do bash "$s"; done\n' > "$root/repo/scripts/run-exit-scripts.sh"
echo notes > "$root/repo/docs/notes.md"
echo '// view' > "$root/repo/clients/swift/View.swift"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm init
git -C "$root/repo" push -q origin main
git -C "$root/repo" checkout -q -b owner-work

# The scripted agent makes a different change per task (the directory is named after the task).
cat > "$root/agent.sh" <<'AGENT'
# Runs in the task's worktree; the directory is named after the task.
g() { git -c user.name=t -c user.email=t@t "$@"; }
cleanup_lines() {
  echo '#!/usr/bin/env bash'
  echo 'kill "$P" 2>/dev/null || true'
  echo 'wait "$P" 2>/dev/null || true'
  echo 'rm -rf "$T" || true'
  echo "trap 'rm -rf \$T || true' EXIT"
  echo 'command -v jq >/dev/null 2>&1 || true'
}
# MASON hands out the number of a task's exit script (s43); the agent reads it from its prompt.
N=$(printf '%s' "$FAKE_PROMPT" | sed -n 's/.*your exit script is scripts\/exit\/s\([0-9]*\)\.sh.*/\1/p' | head -1)
case $(basename "$PWD") in
  0) cleanup_lines > scripts/exit/s$N.sh ;;
  1) cleanup_lines > scripts/exit/s$N.sh; echo "bash scripts/exit/s$N.sh" >> scripts/run-exit-scripts.sh ;;
  2) echo 'cargo test || true' >> scripts/exit/s17.sh ;;
  3) # another task lands a protected change first; this one merges that base in
     g checkout -q -b side; echo 'cargo test || true' >> scripts/exit/s16.sh
     g add -A; g commit -qm landed-first; g push -q origin side:main; g checkout -q -
     echo more >> docs/notes.md; g add -A; g commit -qm mine; g merge -q --no-edit side ;;
  4) echo more >> docs/notes.md; echo '// view 2' >> clients/swift/View.swift ;;
esac
g add -A; g commit -qm change --allow-empty
AGENT
mkdir -p "$root/c/projects"
turn="{\"sh\": \"bash $root/agent.sh\"}, {\"call\": {\"tool\": \"submit_change\", \"args\": {\"summary\": \"done\", \"head\": \"\$HEAD\"}}}"
echo "{\"steps\": {\"develop\": [[$turn]]}}" > "$root/script.json"
cat > "$root/c/projects/mason.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"
claude_bin = "$PWD/$bin/fake-claude"
[self]
max_open = 5
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML
"$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --max-concurrent 4 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== 1. a new exit script with cleanup lines is not held"
m task add "new exit script" --project mason --explored --body "x" >/dev/null   # 0
wait_state 0 landed 90
n0=$(m task show 0 | sed -n 's/^  number    exit \([0-9]*\):.*/\1/p')
on_origin scripts/exit/s$n0.sh
echo "task #0 (a new scripts/exit/s$n0.sh, its reserved number, full of kill/wait/rm/trap ... || true) landed freely"

echo "== 2. a new exit script wired into the runner is held on the runner only"
m task add "wire an exit script" --project mason --explored --body "x" >/dev/null   # 1
wait_held 1 90
m task show 1 | grep 'waiting for your approval'
m self | grep 'task #1' | grep -q 'touches scripts/run-exit-scripts.sh:[0-9]'
n1=$(m task show 1 | sed -n 's/^  number    exit \([0-9]*\):.*/\1/p')
if m self | grep 'task #1' | grep -q "s$n1.sh"; then echo "the new exit script was held by its path" >&2; exit 1; fi
if m self | grep 'task #1' | grep -q 'broadened skip'; then echo "a cleanup line was flagged" >&2; exit 1; fi
m self | grep 'task #1' | grep -q 'Decide: approve only if this change is intended'
m self | grep 'task #1' | grep -q 'Diff: git -C .* diff .*\.\.\.'

echo "== 3. a real \`|| true\` added to an existing gate is held, naming file and line"
m task add "weaken a gate" --project mason --explored --body "x" >/dev/null   # 2
wait_held 2 90
m task show 2 | grep 'waiting for your approval'
m self | grep 'task #2' | grep -q 'broadened skip at scripts/exit/s17.sh:3 (cargo test || true)'
m self | grep 'task #2' | grep -q 'touches scripts/exit/s17.sh:3'

echo "== 4. a task that merges in a base with a protected change is not held for it"
m task add "merge a base" --project mason --explored --body "x" >/dev/null   # 3
wait_state 3 landed 90
echo "task #3 (docs, with a base merged in that edits scripts/exit/s16.sh) landed freely"
git -C "$root/origin.git" show main:scripts/exit/s16.sh | grep -q 'cargo test || true'

echo "== 5. a task touching only clients/swift and docs is never held"
m task add "swift and docs" --project mason --explored --body "x" >/dev/null   # 4
wait_state 4 landed 90
echo "task #4 (clients/swift, docs) landed freely"
for t in 0 3 4; do [ -z "$(m task show $t | grep 'waiting for your approval' || true)" ]; done
if m self | grep -E 'task #(0|3|4) ' | grep -q 'WAITING FOR YOU'; then echo "a harmless task was held" >&2; exit 1; fi
echo "-- task show / self, as the owner reads them:"
m task show 1 | grep 'waiting for your approval'
m task show 2 | grep 'waiting for your approval'
m self

trap - EXIT; cleanup
if [ -z "${MASON_TESTS_ALREADY_RAN:-}" ]; then
  cargo test -q -p mason-daemon --lib protect 2>&1 | grep 'test result'
  cargo test -q -p mason-daemon --test selfguard 2>&1 | grep 'test result'
fi
echo "s41 OK"
