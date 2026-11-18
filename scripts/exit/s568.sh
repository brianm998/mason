#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s568 exit test: a scratch landing slot with stale .rcgu.o sets of two hashes and a reported
# deps size over the limit is renamed aside before the build, which then sees an empty deps;
# under the limit the stale set is pruned. Also: stalled compiles are named by their directory.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib taskcache::tests::stale_sets_go_from_a_landing_slot_and_a_base_directory
cargo test -q -p mason-daemon --lib taskcache::tests::a_directory_over_the_limit_is_renamed_aside_and_removed_once
cargo test -q -p mason-core --lib health::tests::many_compiles_in_uninterruptible_wait_name_their_directory
echo "s568: ok"
