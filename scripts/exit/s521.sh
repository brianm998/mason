#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s521 exit test: every landing batch runs the lint tier, tests skipped or not; a red lint parks the batch.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- lint_places
cargo test -q -p mason-daemon --test it -- lint
grep -q 'name   = "lint"' .mason/landing.toml
grep -q '^\[suite.lint\]' .mason/mason.toml
echo "s521 OK"
