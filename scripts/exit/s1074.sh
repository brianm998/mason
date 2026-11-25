#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1074: neither the runner's fallback watcher (used with no timeout(1)) nor a child a script
# leaves behind may hold the output pipe after the script ends.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
. scripts/lib/bounded.sh
unset MASON_EXIT_SHARD MASON_EXIT_PLAN
root=$(mktemp -d); trap 'rm -rf "$root"' EXIT
mkdir -p "$root/bin" "$root/x" "$root/y"
for t in bash git awk sed grep tee mktemp rm cat tail head cut tr printf basename dirname sleep pkill kill env perl wait; do
  p=$(command -v "$t" 2>/dev/null || true)
  case $p in /*) ln -sf "$p" "$root/bin/$t" ;; esac
done
! [ -e "$root/bin/timeout" ] && ! [ -e "$root/bin/gtimeout" ]
printf '#!/usr/bin/env bash\nexit 0\n' > "$root/x/s01.sh"
printf '#!/usr/bin/env bash\nexit 2\n' > "$root/x/s02.sh"
run() {
  bounded 120 env PATH="$root/bin" MASON_EXIT_DIR="$root/x" MASON_EXIT_WEIGHTS=/dev/null \
    MASON_EXIT_SCRIPT_LIMIT_S=1234 "$(command -v bash)" scripts/run-exit-scripts.sh
}
# a passing script returns promptly
rm "$root/x/s02.sh"
out=$(run 2>&1)
case $out in *"== "*s01.sh*) ;; *) echo "no run: $out"; exit 1 ;; esac
if pgrep -f 'sleep 1234' >/dev/null; then echo "leftover sleep 1234"; pkill -f 'sleep 1234' || true; exit 1; fi
# a failing script is a plain FAIL, not "over"
printf '#!/usr/bin/env bash\nexit 2\n' > "$root/x/s02.sh"
rc=0; out=$(run 2>&1) || rc=$?
[ "$rc" -ne 0 ]
case $out in *"FAILED: "*s02.sh*) ;; *) echo "no FAILED line: $out"; exit 1 ;; esac
case $out in *"(over "*) echo "wrongly reported over"; exit 1 ;; esac
# a child left behind with stdout inherited is reaped, with a limit and without one. The guard
# is only a hang detector: the child would otherwise hold the pipe past it.
printf '#!/usr/bin/env bash\nsleep 600 &\necho $! > "%s"\nexit 0\n' "$root/child" > "$root/y/s04.sh"
for lim in 1234 0; do
  rc=0; out=$(bounded 120 env PATH="$root/bin" MASON_EXIT_DIR="$root/y" MASON_EXIT_WEIGHTS=/dev/null \
    MASON_EXIT_SCRIPT_LIMIT_S=$lim "$(command -v bash)" scripts/run-exit-scripts.sh 2>&1) || rc=$?
  child=$(cat "$root/child"); rm "$root/child"
  if kill -0 "$child" 2>/dev/null; then kill "$child"; echo "leftover sleep 600 (limit $lim)"; exit 1; fi
  [ "$rc" -eq 0 ] || { echo "runner exit $rc (limit $lim): $out"; exit 1; }
  case $out in *"s04: "*" s"*) ;; *) echo "no pass line (limit $lim): $out"; exit 1 ;; esac
done
# a script over the limit is reported over N s
rm "$root/x/s02.sh"
printf '#!/usr/bin/env bash\nsleep 30\n' > "$root/x/s03.sh"
rc=0; out=$(bounded 120 env PATH="$root/bin" MASON_EXIT_DIR="$root/x" MASON_EXIT_WEIGHTS=/dev/null \
  MASON_EXIT_SCRIPT_LIMIT_S=1 "$(command -v bash)" scripts/run-exit-scripts.sh 2>&1) || rc=$?
[ "$rc" -ne 0 ]
case $out in *"(over 1 s)"*) ;; *) echo "not reported over: $out"; exit 1 ;; esac
echo "s1074 OK"
