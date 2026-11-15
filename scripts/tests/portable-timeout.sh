#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# scripts/lib/bounded.sh must bound a command on a machine with no `timeout` binary (stock
# macOS, GitHub's macos-latest). The PATH here holds only perl, sleep, true, false, so the
# GNU timeout is absent. No timing assertions: only exit codes and output.
set -euo pipefail
cd "$(dirname "$0")/../.."
root=$PWD
bin=$(mktemp -d); trap 'rm -rf "$bin"' EXIT
for t in perl sleep true false echo; do ln -s "$(type -P $t)" "$bin/$t"; done
run() { PATH="$bin" "$BASH" -c ". '$root/scripts/lib/bounded.sh'; $1"; }

if PATH="$bin" command -v timeout >/dev/null 2>&1; then echo "test setup: timeout still on PATH" >&2; exit 1; fi
[ "$(run 'bounded 30 echo hi')" = hi ] || { echo "output not passed through" >&2; exit 1; }
run 'bounded 30 true' || { echo "success status lost" >&2; exit 1; }
rc=0; run 'bounded 30 false' || rc=$?
[ "$rc" = 1 ] || { echo "failure status lost: $rc" >&2; exit 1; }
rc=0; run 'bounded 1 sleep 60' || rc=$?
[ "$rc" != 0 ] || { echo "a command over the limit was not stopped" >&2; exit 1; }
echo "portable-timeout ok"
