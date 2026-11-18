#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s546 exit test: a task runs only the test targets its change affects plus the smoke set;
# the whole suite is the landing batch tier's.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q 'name   = "batch"' .mason/landing.toml || { echo "s546: no batch tier" >&2; exit 1; }
grep -q 'suites = \["cargo-workspace"\]' .mason/landing.toml || { echo "s546: the batch tier does not run cargo-workspace" >&2; exit 1; }
grep -q '{tests_for_change}' .mason/mason.toml || { echo "s546: cargo-workspace does not select tests" >&2; exit 1; }
grep -q '^-p ' .mason/smoke-tests.txt || { echo "s546: the smoke list is empty" >&2; exit 1; }
cargo test -p mason-daemon --lib affected
cargo test -p mason-daemon --lib scope
cargo test -p mason-daemon --test it landing_split::
echo "s546: ok"
