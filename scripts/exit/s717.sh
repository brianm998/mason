#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s717 exit test: the impact-map test probes a path no rule matches.
set -uo pipefail
cd "$(dirname "$0")/../.."
if cargo test -q -p mason-definition --test testing_tables masons_own_impact_map_selects_suites_by_path >/dev/null 2>&1; then
  echo "PASS s717"
else
  echo "FAIL s717"
  exit 1
fi
