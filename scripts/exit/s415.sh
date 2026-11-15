#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s415 exit test: the dropped-line audit keeps a landed line the task moved, exactly, into another file it changed.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-landing --lib -- audit::tests
cargo test -q -p mason-landing --test audit
echo "s415 OK"
