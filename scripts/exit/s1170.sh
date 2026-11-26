#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1170 exit test: a landing push waits out a network outage.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1170.md
cargo test -q -p mason-daemon a_dns_failure_is_unreachable_and_a_refusal_is_not
echo "OK s1170"
