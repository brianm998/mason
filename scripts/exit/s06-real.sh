#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# MANUAL. Not run in CI: lands one real task on a scratch repository with the
# real Claude Code (a few cents of haiku). Needs a logged-in `claude`.
#   CLAUDE_BIN=/path/to/claude bash scripts/exit/s06-real.sh
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s06r.XXXXXX); daemon=""
cleanup() {
  [ -n "$daemon" ] && kill "$daemon" 2>/dev/null || true
  sleep 1
  for f in "$root"/d/h/*/host.kv; do
    [ -e "$f" ] || continue
    for k in host_pid child_pid; do pid=$(sed -n "s/^$k=//p" "$f"); kill -9 "-$pid" 2>/dev/null || kill -9 "$pid" 2>/dev/null || true; done
  done
  [ -n "${KEEP:-}" ] && echo "kept $root" || rm -rf "$root"
}
trap cleanup EXIT

claude="${CLAUDE_BIN:-}"
if [ -z "$claude" ]; then
  claude=$(command -v claude 2>/dev/null || true)
  [ -x "$claude" ] || claude=$(ls -d "$HOME/Library/Application Support/Claude/claude-code"/*/*/claude.app/Contents/MacOS/claude 2>/dev/null | sort -V | tail -1)
fi
[ -x "$claude" ] || { echo "cannot find claude; set CLAUDE_BIN" >&2; exit 2; }
"$claude" auth status 2>/dev/null | grep -q '"loggedIn": true' || { echo "claude is not logged in" >&2; exit 2; }

cargo build -q --workspace --bins
bin=target/debug
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }

git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo"
git -C "$root/repo" checkout -q -b main
printf 'hello\n' > "$root/repo/hello.txt"
cat > "$root/repo/CLAUDE.md" <<'MD'
A tiny scratch project. `hello.txt` holds a greeting. Tests: `test "$(cat goodbye.txt)" = goodbye`.
Commit with `git add` and `git commit`; the tree must be clean before you submit.
MD
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm init
git -C "$root/repo" push -q origin main
git -C "$root/repo" checkout -q -b owner-work
mkdir -p "$root/c/projects"
cat > "$root/c/projects/scratch.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "test \"\$(cat goodbye.txt)\" = goodbye"
claude_bin = "$claude"
model = "haiku"
step_timeout_s = 300
TOML

m doctor | grep -v '^warn' || true
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --sweep-ms 3600000 2>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.2; done

m task add "Add goodbye.txt" --body "Add a file goodbye.txt next to hello.txt, containing the single word goodbye followed by a newline."
end=$((SECONDS + 600))
until [ "$(m task show 0 --json | sed -n 's/^  "state": "\(.*\)",$/\1/p')" = landed ]; do
  s=$(m task show 0 --json | sed -n 's/^  "state": "\(.*\)",$/\1/p')
  case "$s" in failed|canceled) m task show 0; m logs 0; exit 1;; esac
  [ $SECONDS -lt $end ] || { echo "timed out in state $s" >&2; m logs 0; exit 1; }
  sleep 2
done
echo "=== task ==="; m task show 0
echo "=== what reached the remote ==="; git -C "$root/origin.git" log --stat --format='%h %s' main | head -12
echo "=== goodbye.txt on main ==="; git -C "$root/origin.git" show main:goodbye.txt
echo "=== logs ==="; m logs 0
echo "=== status ==="; m status
echo "s06-real OK"
