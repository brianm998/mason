#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1251 exit test: an agent whose API is unreachable and then works never shows `failed` (#1257).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1251.md
cargo test -q -p mason-daemon --test it net_outage::an_agent_that_cannot_reach_its_api_twice
echo "OK s1251"
