#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 526 exit test: a running step reads test_timeout_s (and the other mid-step-safe limits)
# from the live registry, so a registration reload reaches it.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -p mason-daemon --test stop a_running_step_reads_limits_changed_by_a_registration_reload
echo "s526 OK"
