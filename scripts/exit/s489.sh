#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s489 exit test: the decision has its measured rows and the prune is wired in.
# The prune unit tests and the backtrace test run in cargo test --workspace.
set -uo pipefail
cd "$(dirname "$0")/../.."
d=docs/decisions/0482-prune-stale-objects.md
for w in unpacked packed off; do
  grep -Eq "^\| $w +\| [0-9]+ " "$d" || { echo "s489: no measured row for $w"; exit 1; }
done
grep -q 'prune_task_objects' crates/daemon/src/suites.rs || { echo "s489: prune not called"; exit 1; }
echo "s489: ok"
