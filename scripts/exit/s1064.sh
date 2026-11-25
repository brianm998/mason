#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1064 exit test: the checks behind the red `exit-scripts (ubuntu-latest, 1)` job pass:
# the Windows type-check, project rename with no USER or MASON_AS, and feature parity.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q '^Summary: ' docs/stages/s1064.md
bash scripts/exit/s193.sh
env -u MASON_AS -u USER bash scripts/exit/s965.sh
cargo run -q -p mason-conformance -- features clients/features.toml
echo "OK s1064"
