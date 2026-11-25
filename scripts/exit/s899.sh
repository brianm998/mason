#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s899 exit test: the 304 test builds its GitHub through source(), and
# source() calls token() before it builds the GitHub.
set -euo pipefail
cd "$(dirname "$0")/../.."
f=crates/daemon/src/remote.rs
awk '/fn a_304_changes_nothing_and_asks_for_nothing_more/{f=1} f&&/source\(/{ok=1;exit} f&&/GitHub::new/{exit} END{exit !ok}' "$f"
awk '/fn source\(/{f=1} f&&/token\(\);/{ok=1;exit} f&&/GitHub::new/{exit} END{exit !ok}' "$f"
