#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s930 exit test (#952): s799 and the suites_schedule tests pass.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
grep -q '^Summary: ' docs/stages/s930.md
bash scripts/exit/s799.sh
cargo test -q -p mason-daemon --test it -- suites_schedule::
