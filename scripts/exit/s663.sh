#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s663 exit test: the fast-tier landing test no longer depends on a 2 s window.
set -euo pipefail
cd "$(dirname "$0")/../.."
! grep -q 'window = \\"2s\\"' crates/daemon/tests/landing_fast_tier.rs
echo "s663 OK"
