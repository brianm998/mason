#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s732 exit test: abandon_batches names its batches; batch host ids parse.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -p mason-store --test abandon
cargo test -p mason-daemon --lib sweep
