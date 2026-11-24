#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1023 exit test: notification-details has a platform_reason and the feature check passes.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

grep -q '^Summary: ' docs/stages/s1023.md
awk '/^\[feature.notification-details\]/{f=1;next} /^\[/{f=0} f' clients/features.toml | grep -q '^platform_reason = "'
cargo run -q -p mason-conformance -- features clients/features.toml
echo "OK s1023"
