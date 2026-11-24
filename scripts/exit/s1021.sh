#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1021 exit test: notification-details records a platform_reason (decision 1014).
set -euo pipefail
cd "$(dirname "$0")/../.."
sed -n '/^\[feature.notification-details\]/,/^$/p' clients/features.toml | grep -q '^platform_reason = "..*"'
cargo run -q -p mason-conformance -- features clients/features.toml
