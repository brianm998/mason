#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s512 exit test: reds of one class on one base commit file one fix task.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- basecheck::tests
cargo test -q -p mason-daemon --test base_red_group
echo "s512 OK"
