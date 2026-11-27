#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1204 exit test: the mDNS advertisement never claims the machine's own name (decision 1194).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1204.md
cargo test -q -p mason-daemon --lib dnssd::
cargo test -q -p mason-daemon --lib hostname_tests
echo "OK s1204"
