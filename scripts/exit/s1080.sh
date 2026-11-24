#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
set -euo pipefail
cargo test -p conformance
bash scripts/exit/s13.sh
