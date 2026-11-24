#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1076 exit test: without timeout(1), the runner returns when a script ends and
# still kills a script over the limit. No clock assertions: a hang is the failure.
set -euo pipefail
cd "$(dirname "$0")/../.."
repo=$(pwd)
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
mkdir "$tmp/bin" "$tmp/fx"
for t in bash env git mktemp rm cat tee sleep kill pkill pgrep awk sed grep head tail cut tr basename dirname true false; do
  p=$(command -v "$t" 2>/dev/null || true)
  [ -n "$p" ] && [ -x "$p" ] && ln -s "$p" "$tmp/bin/$t"
done
echo "== a passing script returns at once"
printf '#!/usr/bin/env bash\necho fast OK\n' > "$tmp/fx/s11.sh"
env -u MASON_EXIT_SHARD -u MASON_EXIT_PLAN MASON_EXIT_SCRIPT_LIMIT_S=600 PATH="$tmp/bin" MASON_EXIT_DIR="$tmp/fx" MASON_EXIT_WEIGHTS=/dev/null \
  bash scripts/run-exit-scripts.sh >"$tmp/out1" 2>&1 || { cat "$tmp/out1"; echo "FAILED: fast script" >&2; exit 1; }
grep -q 'fast OK' "$tmp/out1" || { cat "$tmp/out1"; echo "FAILED: the fixture did not run" >&2; exit 1; }

echo "== a script over the limit is killed with its children"
marker=s1076-marker-$$
printf '#!/usr/bin/env bash\nbash -c "sleep 600 # %s"\n' "$marker" > "$tmp/fx/s11.sh"
rc=0
env -u MASON_EXIT_SHARD -u MASON_EXIT_PLAN MASON_EXIT_SCRIPT_LIMIT_S=1 PATH="$tmp/bin" MASON_EXIT_DIR="$tmp/fx" MASON_EXIT_WEIGHTS=/dev/null \
  bash scripts/run-exit-scripts.sh >"$tmp/out2" 2>&1 || rc=$?
[ "$rc" -eq 1 ] || { ls -l "$tmp/bin" "$tmp/fx"; cat "$tmp/out2"; echo "FAILED: expected exit 1, got $rc" >&2; exit 1; }
grep -q '(over 1 s)' "$tmp/out2" || { cat "$tmp/out2"; echo "FAILED: no over-limit message" >&2; exit 1; }
if pgrep -f "$marker" >/dev/null 2>&1; then
  pkill -f "$marker" || true
  echo "FAILED: the script's child survived" >&2; exit 1
fi
echo "s1076 OK"
