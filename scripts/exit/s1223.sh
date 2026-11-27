#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1223 exit test: Add Project resolves the agent binary and tokens as the daemon sees them.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1223.md
cargo test -q -p mason-onboard
cargo test -q -p mason-daemon --test it onboarding
echo "OK s1223"
