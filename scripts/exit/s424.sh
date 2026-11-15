#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s424 exit test: a command host that did not come up is started again.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- hostloss::tests::a_host_that_did_not_come_up_is_started_again
echo "s424 OK"
