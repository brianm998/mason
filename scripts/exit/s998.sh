#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s998 exit test: the impact-map test counts the unit-level suites (decision 0991).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s998.md
cargo test -q -p mason-definition --test testing_tables masons_own_impact_map_selects_suites_by_path
echo "OK s998"
