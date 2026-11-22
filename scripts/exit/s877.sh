#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s877 exit test: an analyst result is read once and ends the task; the digest's flaky map is per project.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --test it -- introspection_policy::
echo "s877 passed"
