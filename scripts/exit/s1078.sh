#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1078 exit test (#1088): without timeout(1), run-exit-scripts.sh does not leave the watcher's
# sleep holding the output pipe, so the next script starts at once. Outcomes only.
set -euo pipefail
repo=$(cd "$(dirname "$0")/../.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
fail() { echo "s1078 FAIL: $*" >&2; exit 1; }
w="$tmp/w"; bin="$tmp/bin"
mkdir -p "$w/scripts/exit" "$bin"
for t in bash sh env dirname basename mktemp rm cat tee sleep kill pkill pgrep sed grep head tail cut tr sort date git awk ls wc uniq printf expr python3 xargs find ps; do
  p=$(command -v "$t" 2>/dev/null || true)
  case $p in /*) ln -sf "$p" "$bin/$t" ;; esac
done
printf '#!/usr/bin/env bash\nexit 0\n' > "$w/scripts/exit/s10.sh"
printf '#!/usr/bin/env bash\nexit 0\n' > "$w/scripts/exit/s11.sh"
printf '#!/usr/bin/env bash\nexit 1\n' > "$w/scripts/exit/s12.sh"
cp "$repo/scripts/run-exit-scripts.sh" "$w/scripts/"
cd "$w"
git init -q .; git config user.email t@example.invalid; git config user.name t
git config commit.gpgsign false
git add -A; git commit -q -m base
limit=3571
set +e
out=$(PATH="$bin" MASON_EXIT_SCRIPT_LIMIT_S=$limit MASON_EXIT_SHARD= bash scripts/run-exit-scripts.sh 2>&1 | cat)
set -e
echo "$out" | grep -q '^s10: ' || fail "s10 did not finish: $out"
echo "$out" | grep -q '^s11: ' || fail "s11 did not finish: $out"
echo "$out" | grep -q 'FAILED: scripts/exit/s12.sh' || fail "the failing script was not reported: $out"
if ps -axo command | grep -q "[s]leep $limit"; then
  pkill -f "sleep $limit" || true
  fail "a watcher sleep survived the run"
fi
echo "s1078 OK"
