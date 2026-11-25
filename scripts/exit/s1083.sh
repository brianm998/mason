#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 1083 exit test: a silent host waiting on a resource is not stuck:host.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -p mason-core health::
