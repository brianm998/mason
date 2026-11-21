#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s741 exit test: scripts/test-shard.py is mapped in [impact].
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-cli definition::tests::no_tracked_file_but_the_definition_is_unmapped
cargo test -q -p mason-cli definition::tests::the_paths_that_ran_everything_are_now_mapped
echo "s741 OK"
