#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s781 exit test: the idle debt pass runs each suite once per tip, backs off after a red, and
# a develop CI run with every mapped job green records the suite green and settles debts.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
bash scripts/check-exit-guards.sh
grep -q '^Summary: ' docs/stages/s781.md
cargo test -q -p mason-daemon --test it -- \
  three_debts_of_different_scopes_on_one_tip \
  after_a_red_a_new_tip_runs_the_suite_only_when_it_touched_a_red_module \
  a_ci_run_with_every_mapped_job_green
echo "s781 OK"
