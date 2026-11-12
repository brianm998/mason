#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 174 exit test: a red landing batch is compared with the base branch's own CI.
#  1. The pure rules (core): pytest and cargo failures are read; the same tests with the same
#     text are "red on base"; other text is held for the owner; a test or job green on the base,
#     or a base that cannot be read, still blames the batch.
#  2. Over the base branch's runs as the CI host serves them (the `Source` the watch uses):
#     a red base and a red candidate failing the same test: not blamed; a candidate red on a job
#     that is green on the base: blamed; the same test with other text: held, with both texts.
#  3. The GitHub adapter keeps the end of a failed job's log, with the judge on or off.
# This does not start a masond or serve a fake GitHub over HTTP; see docs/stages/s174.md.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-core red_base
cargo test -q -p mason-daemon --test red_base
cargo test -q -p mason-adapters --test github -- with_the_judge_off
echo "s174 OK"
