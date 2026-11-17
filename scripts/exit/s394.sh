#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 394 exit test: a red batch that blame cannot pin is split to find who breaks it.
#  1. Three tasks, one breaks a suite: the other two land, the one goes back with the red,
#     and the batch's note shows the split (real masond, real git, real suites).
#  2. Two tasks that break it only together: both go back, named together; the third lands.
#  3. The pure search: one culprit, an interaction, independent culprits, a base red.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-landing --lib pin
cargo test -q -p mason-daemon --test it landing_split::
echo "s394 OK"
