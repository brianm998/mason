#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s337 exit test: a failed task can be canceled, and revived afterwards.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-engine --test behaviour -- a_failed_task_made_obsolete_is_canceled_and_can_still_be_revived cancel_from_every_non_terminal_state_stops_and_releases
cargo test -q -p mason-engine --test matrix
echo "s337 OK"
