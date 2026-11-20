#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s684 exit test: an impact entry `{ suites = [], override = true }` replaces a broader list, so
# scripts/exit/s1.sh selects nothing while scripts/x.sh still selects the suite.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-definition --test testing_tables -- an_override_entry_replaces_broader_impact_entries impact_tables_name_their_mistakes a_dead_empty_impact_entry_warns
cargo test -q -p mason-testing -- override specificity
echo "PASS s684"
