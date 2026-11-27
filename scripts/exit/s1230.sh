#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1230 exit test: the lost-host test waits for the host's record before scanning (decision 1220).
set -euo pipefail
cd "$(dirname "$0")/../.."

cargo test -q -p mason-daemon --test nodes a_node_job_whose_host_dies_leaves_no_grandchild_behind
echo "OK s1230"
