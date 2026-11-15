#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s355 exit test: a run that ended within its limit is not timed out, however late it is collected.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-step-host --lib -- a_command_that_ended_within_its_limit_did_not_run_past_it
echo "s355 OK"
