#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Every workflow parses and ci.yml keeps the CI: full run-name (s808, decision 0801).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
bash scripts/check-workflows.sh
grep -q '^run-name: .*CI: full' .github/workflows/ci.yml || { echo "FAILED: run-name lost CI: full" >&2; exit 1; }
cargo test -q -p mason-conformance --test ci_gates
echo "s808 ok"
