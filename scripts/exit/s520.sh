#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s520 exit test: run_check marks failing tests that are red on develop too.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- knownred::tests::a_test_red_on_develop_is_named
cargo test -q -p mason-store --test store -- the_latest_baseline_of_a_suite
echo "s520 OK"
