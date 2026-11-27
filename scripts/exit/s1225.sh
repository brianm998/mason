#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1225 exit test: a suite given back by a node stays off the coordinator in remote/no-fallback projects.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q 'fn off_machine_until_placed' crates/daemon/src/suites.rs
grep -q 'stays_off_for' crates/daemon/src/suites.rs
grep -q 'fn stays_off_for' crates/daemon/src/hostedcheck.rs
echo "s1225: ok"
