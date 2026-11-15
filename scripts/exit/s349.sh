#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s349 exit test: an aged request never passes work further along.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-scheduler --lib -- an_aged_request_goes_ahead_of_pipeline_work_oldest_first_but_not_ahead_of_a_landing
cargo test -q -p mason-scheduler --test properties -- aging an_exclusive_request_at_the_back
echo "s349 OK"
