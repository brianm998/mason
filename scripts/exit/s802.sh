#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s802 exit test: stage note present and the floor tests and config key still exist (grep only).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
grep -q '^Summary: ' docs/stages/s802.md
for t in the_floor_builds_here_while_the_switch_says_github_and_lands \
  a_floor_set_to_github_goes_there a_floor_red_built_here_parks_the_task; do
  grep -q "fn $t" crates/daemon/tests/it/landing_floor.rs
done
grep -q 'floor_where' crates/daemon/src/landing.rs
grep -q 'floor_where' crates/landing/src/config.rs
echo ok
