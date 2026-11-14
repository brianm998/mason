#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s235 exit test: log_signature lists only the task's own hosts.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test hostarchive log_signature_names_only
