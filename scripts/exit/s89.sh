#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s89 exit test: the repository's own landing.toml has a 10 min window (shortened to 30 s in s89, lengthened again since).
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q '^window   = "10m"' .mason/landing.toml
