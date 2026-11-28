#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1318 exit test: update health rows name the target and the error (decision 1308).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1318.md
cargo test -q -p mason-daemon --test it a_failed_update_is_logged_with_its_target_and_error
echo "OK s1318"
