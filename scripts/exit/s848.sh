#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s848 exit test: the account's queued and macOS jobs count against the pools (#819).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --lib -- queued_jobs_in_other_repositories macos_jobs a_304_changes_nothing a_repository_the_token_cannot_read a_failed_look_keeps
echo "s848 passed"
