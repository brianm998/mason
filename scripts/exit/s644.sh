#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s644 exit test: the ten-compiles test checks task ids, not a clock-dependent string.
set -euo pipefail
cd "$(dirname "$0")/../.."
for _ in 1 2 3; do
  cargo test -q -p mason-daemon --lib -- sched::tests::ten_compiles_behind_a_waiting_exclusive_lease_all_name_it
done
cargo test -q -p mason-daemon --lib -- sched::tests
! grep -q 'format!("{:?}", v.tasks)' crates/daemon/src/sched.rs
echo "s644 OK"
