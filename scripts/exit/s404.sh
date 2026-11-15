#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s404 exit test: the report's unit test over a synthetic event log with known counts.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib report::tests
