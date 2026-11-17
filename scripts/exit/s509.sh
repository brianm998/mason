#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s509 exit test: a skipped-test batch runs the fast unit tier; a merge that breaks a unit test is parked naming it.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test landing_fast_tier
# The tier the project file declares is read, and the matrix test it covers passes.
cargo test -q -p mason-definition --test fast_tier
cargo test -q -p mason-engine --test matrix
echo "s509 OK"
