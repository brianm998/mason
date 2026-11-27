#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1183 exit test: a finished batch's integration branch is deleted on a later pass.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1183.md
cargo test -q -p mason-daemon --lib a_branch_held_by_a_git_lock_is_deleted_on_a_later_pass
echo "OK s1183"
