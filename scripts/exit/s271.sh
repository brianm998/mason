#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s271 exit test: paying a debt writes in the project's test command.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo build -q --workspace --bins
cargo test -q -p mason-daemon --test testing -- paying_a_debt_writes_in_the_projects_test_command
echo "s271 OK"
