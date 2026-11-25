#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1069 exit test: a remote run gives back a runner unit as each GitHub job finishes (#820).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1069.md
cargo test -q -p mason-scheduler shrinking_a_holder_frees_exactly_those_units
cargo test -q -p mason-daemon --lib a_finished_shard_gives_its_unit_back_at_once
cargo test -q -p mason-daemon --lib shards_run_again_ask_for_their_units_again
cargo test -q -p mason-daemon --lib a_refused_or_fallen_back_run_with_an_emptied_lease
echo "OK s1069"
