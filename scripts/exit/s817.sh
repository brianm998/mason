#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Debt payments rank behind all task work for slots and runner units (s817, decision 0810):
# the scheduler's debts band, and a debt's fallback run behind a live task's request.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-scheduler a_debt_goes_after
cargo test -q -p mason-daemon --test it fallback_lease
echo "s817 ok"
