#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s403 exit test: a build failure is named by path:line or "build", never by a line of code.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-testing --lib -- build_failure_names
echo "s403 OK"
