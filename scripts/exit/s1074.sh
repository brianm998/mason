#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1074: on a machine with no timeout(1) the runner's fallback watcher must not leave a
# `sleep` holding the output pipe after a script passes.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
. scripts/lib/bounded.sh
root=$(mktemp -d); trap 'rm -rf "$root"' EXIT
mkdir -p "$root/bin" "$root/x"
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
# a script over the limit is reported over N s
rm "$root/x/s02.sh"
printf '#!/usr/bin/env bash\nsleep 30\n' > "$root/x/s03.sh"
rc=0; out=$(bounded 120 env PATH="$root/bin" MASON_EXIT_DIR="$root/x" MASON_EXIT_WEIGHTS=/dev/null \
  MASON_EXIT_SCRIPT_LIMIT_S=1 "$(command -v bash)" scripts/run-exit-scripts.sh 2>&1) || rc=$?
[ "$rc" -ne 0 ]
case $out in *"(over 1 s)"*) ;; *) echo "not reported over: $out"; exit 1 ;; esac
echo "s1074 OK"
