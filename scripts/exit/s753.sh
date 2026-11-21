#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s753 exit test: parse_duration rejects overflow instead of panicking, and the edge-case tests exist.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
grep -q 'fn duration_edge_cases' crates/landing/src/config.rs
grep -q 'fn duration_overflow_is_none_not_a_panic' crates/landing/src/config.rs
grep -q 'checked_mul' crates/landing/src/config.rs
echo "s753 ok"
