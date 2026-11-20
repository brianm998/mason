#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s731 exit test: mason remote routes setup and on/off/status separately.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -p mason-pipeline --test remote_dispatch
