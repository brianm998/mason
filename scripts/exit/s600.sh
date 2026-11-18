#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 600 exit test: the sweep deletes expired overrides; the detector ignores quarantines.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -p mason-daemon --test it -- an_expired_override_is_deleted
cargo test -p mason-store --test store -- expired_switches
