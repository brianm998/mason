#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s263 exit test: a skip ends a suite's wait for its build slot.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo build -q --workspace --bins
cargo test -q -p mason-daemon --test testing -- a_skip_ends_the_wait_for_a_build_slot_and_takes_no_slot
echo "s263 OK"
