#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s266 exit test: shell discovery, jobs through the shell function, doctor's shell line.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-platform shell::
cargo test -q -p mason-cli doctor_reports_the_shell
