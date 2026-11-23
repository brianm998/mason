#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# An owner's retry note reaches the reviewer and goes when the task lands; an automatic
# retry's note is not kept (task 988, decision 0968).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --lib -- ownernotes::tests
cargo test -q -p mason-daemon --test it -- owner_notes::
echo "s975: all checks passed"
