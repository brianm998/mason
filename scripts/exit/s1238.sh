#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1238 exit test: a subdirectory package gets a suite, a merge with a remote warns and files no
# proof task until pushed, and `mason init --prove-only` goes on after the push.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1238.md
cargo test -q -p mason-onboard
cargo test -q -p mason-pipeline --test init a_merge_that_is_not_pushed
echo "OK s1238"
