#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s322 exit test: a running batch and a queued task each get a landing status line
# (batch, phase, queue position and why).
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib batch_phase_tests
