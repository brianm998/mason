#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s635 exit test: Land now on a running batch lands it past slow testing and the skipped
# tests run after the merge.
#  1. The integration tests start a test daemon with a scratch project whose batch tier suite
#     blocks until a file appears, file two tasks into one running batch, and send
#     `POST /batches/{id}/land-now` over the API. They check that both tasks reach `landed`,
#     that each owes one deferred `suite:` debt, that releasing the blocked suite pays both on
#     the base tip, that a red result leaves the debt owed and raises the alert, that a batch
#     that does not build is not landed, and that a freeze is kept.
#  2. The unit tests: `landing:force` / `landing:land_now` debts resolve to the tier's suites,
#     and the views offer `land_now` only until a batch starts promoting.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
echo "== 1. land now over the API, against a blocked batch tier"
cargo test -q -p mason-daemon --test it land_now
echo "== 2. debts and views"
cargo test -q -p mason-daemon --lib land_now_debt_tests
cargo test -q -p mason-daemon --lib batch_phase_tests
echo "s635 passed"
