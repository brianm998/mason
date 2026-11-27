#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1185 exit test: a full CI run that was cancelled, or whose commit develop has left, is not
# rerun (decision 1175).
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test it ci_schedule::
