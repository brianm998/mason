#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s479 exit test: the leak test passes and no scratch-world process is left.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --test leaks
left=$(/bin/ps -axo command= | grep -E -- '--(data-)?dir (/private)?/tmp/ms[A-Za-z0-9]+/' | grep -v grep || true)
[ -z "$left" ] || { echo "leftover: $left"; exit 1; }
