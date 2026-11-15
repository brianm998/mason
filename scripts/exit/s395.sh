#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s395 exit test: a landed line the task rewrites in place is paired for review; a plain loss is still dropped.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-landing --test audit
cargo test -q -p mason-landing --lib -- audit::tests
echo "s395 OK"
