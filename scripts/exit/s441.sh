#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s441 exit test: the workspace is rustfmt-clean and the CI fmt gate passes.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo fmt --all --check
cargo test -q -p mason-conformance --test ci_gates workspace_is_fmt_clean
echo "s441 passed"
