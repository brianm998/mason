#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s632 exit test: narrowing drops the paths the impact map sends to other suites (#616).
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib paths_the_impact_map_sends_elsewhere
cargo test -q -p mason-daemon --lib no_path_left_runs_the_smoke_set
cargo test -q -p mason-daemon --lib the_smoke_file_alone
