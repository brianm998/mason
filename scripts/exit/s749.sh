#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s749 exit test: a run on GitHub shows what it is doing while it goes (#760): the phase from
# the jobs API, and for a sharded run how many shards are done. Runs the new tests by name.
set -euo pipefail
cd "$(dirname "$0")/../.."
for t in \
  the_phase_of_a_run_in_progress \
  a_sharded_run_says_how_many_shards \
  a_run_in_progress_reports_its_phase; do
  cargo test -q -p mason-daemon --lib "remote::tests::$t"
done
echo "s749: green"
