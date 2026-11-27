#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1212 exit test: the command-guard settings test holds on Windows too (decision 1202).
set -euo pipefail
cd "$(dirname "$0")/../.."

cargo test -q -p mason-adapters --lib every_session_is_given_the_command_guard
echo "OK s1212"
