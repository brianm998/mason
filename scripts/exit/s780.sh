#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s780 exit test: remote runs share one warm cache saved only from the base branch (#770).
# Checks the workflow text, run.json's toolchains and the cache DELETE on the sweep path.
# Not checkable here: `Cache restored` in a real run's log, which needs the warm-up to have run.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q 'shared-key: mason-run' .github/workflows/mason-run.yml
grep -q "save-if: \${{ github.ref == 'refs/heads/develop' }}" .github/workflows/mason-run.yml
grep -q 'shared-key: mason-run' .github/workflows/mason-cache.yml
grep -q -- '--no-run' .github/workflows/mason-cache.yml
cargo test -q -p mason-daemon --lib remote::tests::run_json_names_the_toolchains
cargo test -q -p mason-daemon --lib remote::tests::a_green_run_gives_its_output
cargo test -q -p mason-pipeline --bin mason the_cache_usage_warns
echo "s780: ok"
