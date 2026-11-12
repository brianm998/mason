#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s181 exit test: the wait_share finding lists what was waited on.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -p mason-core introspect::tests::wait_breakdown_orders_by_share
