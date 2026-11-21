#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s756 exit test: parked agents do not hold LLM sessions back (#765).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --lib park_tests
cargo test -q -p mason-definition park_extra
