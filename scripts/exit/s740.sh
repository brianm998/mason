#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s740 exit test: the thermal limits test has enough compile slots.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -p mason-daemon --test it limits::a_hot_die_holds_further_machine_work_but_never_what_runs
