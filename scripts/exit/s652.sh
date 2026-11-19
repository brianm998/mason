#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s652 exit test: the resume tests pass, including the cut-off-in-the-third-executable case.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon resume::tests || { echo "FAILED: resume tests" >&2; exit 1; }
echo "s652 OK"
