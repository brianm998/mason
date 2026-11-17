#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s479 exit test: dropping a scratch world leaves none of its processes.
# The test itself lists processes whose command line holds its own temp dir and
# fails if any remain; other tasks' worlds on this machine are not looked at.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --test leaks
