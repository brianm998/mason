#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s94 exit test: a frozen step-host launch is waited for, not failed.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-step-host --test host -- the_start_limit a_launch_that_freezes
echo "s94 OK"
