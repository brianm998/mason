#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s760 exit test: adopted GitHub runs that finished or that nobody can collect free their units.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib remote::tests:: -- \
  a_hold_whose_run_completed a_hold_whose_run_is_still_going an_unwanted_run \
  checks_of_a_landed_task
