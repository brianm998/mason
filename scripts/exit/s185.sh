#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Suite commands are told what they hold (s185).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --test scheduling -- told what
cargo test -q -p mason-daemon --test scheduling -- exclusive_suite_lease
