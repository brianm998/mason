#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1077: without timeout(1) the runner must not wait for its watcher's sleep.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$PWD
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
mkdir "$tmp/bin" "$tmp/scripts"
for t in bash sed grep awk tee mktemp basename tail cut tr sleep pkill pgrep kill git rm head cat dirname env printf; do
  p=$(command -v "$t" || true)
  [ -n "$p" ] && [ -x "$p" ] && ln -sf "$p" "$tmp/bin/$t"
done
printf '#!/usr/bin/env bash\ntrue\n' > "$tmp/scripts/s01.sh"
out=$(cd "$root" && PATH="$tmp/bin" MASON_EXIT_DIR="$tmp/scripts" MASON_EXIT_SCRIPT_LIMIT_S=7717 \
  bash scripts/run-exit-scripts.sh 2>&1)
echo "$out" | grep -q '^s01: [0-9]* s$' || { echo "no timing line: $out" >&2; exit 1; }
if pgrep -f 'sleep 7717' >/dev/null; then echo "leftover sleep 7717" >&2; exit 1; fi
echo "s1077 OK"
