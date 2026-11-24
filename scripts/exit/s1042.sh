#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1042 exit test: a recovered step keeps its tree (decision 1035).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1042.md
grep -q 'prepared-r' crates/daemon/src/jobs.rs
cargo test -q -p mason-daemon --test it a_recovered_develop_step_keeps_its_tree_when_the_base_moved
echo "OK s1042"
