#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s649 exit test: per-executable times parsed from a build log and kept in the store, and the
# lane rule (two short recorded targets are small; a long, unrecorded or whole selection is not).
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -p mason-store --test test_times target_times
cargo test -p mason-daemon --lib targettimes
cargo test -p mason-daemon --lib small_lane
echo "s649 ok"
