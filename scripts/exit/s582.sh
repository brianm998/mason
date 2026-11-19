#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s582 exit test: the world limit holds across processes and git resolves to the real binary.
# Does not measure the xcrun-shim exec rate (see docs/stages/s582.md).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
[ -n "${MASON_TESTS_ALREADY_RAN:-}" ] || {
  cargo test -q -p mason-daemon --test world_slots
  cargo test -q -p mason-landing program_tests
}
