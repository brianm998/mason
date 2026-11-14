#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 213 exit test: a CI red confined to jobs that map to local suites (`[ci.local]` in
# landing.toml) is searched for with those suites, not with more CI runs.
#  1. The config parses `[ci.local]` and rejects malformed ones.
#  2. A 4-task batch red in a mapped job, one culprit: the culprit is found locally and the
#     fake CI is dispatched exactly twice (the batch, then the union without the culprit).
#  3. Red in an unmapped job still bisects through CI (three or more dispatches).
#  4. Mapped suites green here while CI is red: the owner is asked ("environment difference"),
#     exactly one dispatch, nobody landed or sent back.
#  5. A job red in the base's latest run is named by `base_red_jobs` (held, not bisected).
# The dispatches are counted by the tests from the fake CI's trigger log.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-landing --lib ci_local
cargo test -q -p mason-daemon --test landing_modes -- ci_red_in_a_mapped ci_red_in_an_unmapped mapped_suites_green
cargo test -q -p mason-daemon --test red_base -- base_red_jobs red_in_the_latest_base green_missing
echo "s213 OK"
