#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s524 exit test: MASON's own .mason/mason.toml turns on [watch.ci] with the freeze off.
set -euo pipefail
cd "$(dirname "$0")/../.."
f=.mason/mason.toml
sec=$(awk '/^\[watch\.ci\]/{on=1;next} /^\[/{on=0} on' "$f")
echo "$sec" | grep -Eq '^enabled *= *true' || { echo "FAIL: watch.ci not enabled"; exit 1; }
echo "$sec" | grep -Eq '^freeze_on_gate *= *false' || { echo "FAIL: freeze not off"; exit 1; }
cargo test -q -p mason-definition --test testing_tables masons_own_ci_watch_is_on_with_no_freeze
echo "s524 ok"
