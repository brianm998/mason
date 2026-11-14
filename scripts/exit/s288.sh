#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s288 exit test: the wait_share threshold is still 0.6 and the decision is recorded.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q 'wait_share: 0.6,' crates/core/src/introspect.rs
test -f docs/decisions/0283-wait-share-is-the-exclusive-queue.md
echo "s288 OK"
