#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s89 exit test: the repository's own landing.toml has a 30 s window.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q '^window   = "30s"' .mason/landing.toml
