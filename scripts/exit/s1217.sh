#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1217 exit test: a stuck:long finding names the task's open remote runs (decision 1207).
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-core health::
cargo test -q -p mason-daemon --test it health::
