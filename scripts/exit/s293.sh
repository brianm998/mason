#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s293 exit test: a gate step validates under what the daemon can run, while `ci` is still
# refused.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-definition --test validate what_cannot_run_yet
