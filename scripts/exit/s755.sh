#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s755 exit test: swift-clients is configured for a macOS runner.
set -euo pipefail
cd "$(dirname "$0")/../.."
awk '/^\[suite.swift-clients\]/{f=1;next} /^\[/{f=0} f&&/^runs_on = "macos-latest"/{ok=1} END{exit !ok}' .mason/mason.toml
echo "s755: ok"
