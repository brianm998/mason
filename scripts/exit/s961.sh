#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s961 exit test: the transport tests and the gated daemon code still pass on unix.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-step-host --test portable
cargo test -q -p mason-daemon --lib -- sweep:: remote::
echo "s961 passed"
