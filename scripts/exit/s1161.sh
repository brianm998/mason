#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 1161 exit test: a launch that fails before it is listed reports why.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-step-host --test host -- a_launch_refused_before_it_is_listed
cargo test -q -p mason-step-host --test host -- a_launch_is_
