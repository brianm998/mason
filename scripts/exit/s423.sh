#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s423 exit test: a conflict send-back names the base commits that changed the conflicting files.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-landing --test batch -- a_task_conflicting_with_the_base_is_ejected
echo "s423 OK"
