#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s429 exit test: the audit reads other files only when a line is missing; a skipped batch is rebuilt, not re-tested.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-landing --lib -- audit::tests
cargo test -q -p mason-landing --test audit
cargo test -q -p mason-daemon --test it -- retest a_batch_whose_testing_was_skipped_is_rebuilt_but_not_retested_when_the_base_moves
echo "s429 OK"
