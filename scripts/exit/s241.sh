#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s241 exit test: a fake release is downloaded, verified and self-tested by `mason update --check`.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test update_release
cargo test -q -p mason-daemon --lib release
echo "s241: release channel tests green"
