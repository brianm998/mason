#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s290 exit test: the project file's settings are listed.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --lib -- the_project_files_own_settings_are_listed_with_an_off_zero_named
echo "s290 OK"
