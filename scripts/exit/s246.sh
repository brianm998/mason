#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s246 exit test: a red the base has too is not the task's. A fake suite is vacuous (or does not
# build) at the base and on the branch: the task waits in its test step with wait class
# `base-red`, one task is filed, and when the base is mended the step runs again and passes; a
# red the base does not have still sends the task back.
set -euo pipefail
cd "$(dirname "$0")/../.."

run() {
    local what=$1; shift
    echo "== $what"
    out=$(cargo test -q "$@" 2>&1) || { echo "$out" >&2; echo "FAILED: $what" >&2; exit 1; }
    echo "$out" | grep -E '^test result' | tail -3
}

run "the normalising and comparing of first errors" -p mason-testing --lib basered
run "the daemon: base-red waits, filing, clearing, and a red the base lacks" \
    -p mason-daemon --test base_red
echo "s246 OK"
