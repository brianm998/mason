#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s298 exit test: no tracked sed backup files.
set -uo pipefail
cd "$(dirname "$0")/../.."
if git ls-files | grep -- '-e$'; then echo "s298: stray -e files"; exit 1; fi
echo "s298: no stray -e files"
