#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1305 exit test: a dispatch lost to an outage keeps its ref and a cancel_pending record; a
# refused token leaves neither (decision 1295).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1305.md
cargo test -q -p mason-daemon --lib remote::tests::an_outage_or_a_refused_token_is_environmental
echo "OK s1305"
