#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s776 exit test: a dead step host whose pid was reused reads as Lost, an orphan host is
# reported once, and the daemon reaps exited children nobody waited for.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -p mason-step-host --lib -- status_tests
cargo test -p mason-daemon --lib -- an_orphan_host_is_reported_once an_exited_child_nobody_waits_for_is_reaped
echo "s776 OK"
