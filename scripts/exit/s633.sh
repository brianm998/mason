#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s633 exit test: after a lost host only the unfinished cargo test executables run again.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- resume::tests
cargo test -q -p mason-daemon --test it -- a_lost_host_reruns_only_the_executables_that_had_not_finished
echo "s633 OK"
