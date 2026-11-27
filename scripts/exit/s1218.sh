#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1218 exit test: a full-CI dispatch is deferred over its share of the gha pool (decision 1208).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1218.md
cargo test -q -p mason-daemon --lib share_tests
cargo test -q -p mason-daemon --lib gha_task_waiting_leaves_out
cargo test -q -p mason-daemon --lib the_full_share_is_a_percent
cargo test -q -p mason-daemon --test it a_full_run_is_deferred
echo "OK s1218"
