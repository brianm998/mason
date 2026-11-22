#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s899 exit test: the 304 test calls token() before it builds its GitHub.
set -euo pipefail
cd "$(dirname "$0")/../.."
awk '/fn other_workflows_are_counted_and_a_304/{f=1} f&&/token\(\);/{ok=1;exit} f&&/GitHub::new/{exit} END{exit !ok}' crates/daemon/src/remote.rs
