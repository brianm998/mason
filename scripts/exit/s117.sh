#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s117 exit test: the adapters and daemon still build and their unit tests pass.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-adapters -p mason-daemon --lib
