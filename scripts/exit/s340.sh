#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s340 exit test: a test step holds `exclusive` only while an exclusive suite runs.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test scheduling -- a_step_with_an_exclusive_suite_holds_the_machine_only_for_that_suite
echo "s340 OK"
