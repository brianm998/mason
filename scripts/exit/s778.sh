#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s778 exit test: clippy in the test step (#773, decision 0771).
#  1. The impact-map and parser tests.
#  2. scripts/clippy-suite.sh, narrowed to one package, reports one passing line.
set -euo pipefail
cd "$(dirname "$0")/../.."

echo "== impact map and parser"
cargo test -q -p mason-definition --test testing_tables masons_own_impact_map_selects_suites_by_path
cargo test -q -p mason-testing --test parsers the_clippy_suite_output

echo "== narrowed clippy over one package"
out=$(bash scripts/clippy-suite.sh -p mason-testing)
echo "$out"
echo "$out" | grep -qx 'test clippy_clean ... ok'
echo "OK s778"
