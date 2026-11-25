#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1080 exit test: the expected dump waits for trailing views (decision 1069).
set -euo pipefail
cargo test -p mason-conformance
bash scripts/exit/s13.sh
