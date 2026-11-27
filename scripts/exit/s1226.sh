#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# A scripted reviewer's invented finding is rejected by a scripted validator: the task lands with
# the finding as a minor note marked "not confirmed" (s1226).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --test it review_validate::
cargo test -q -p mason-daemon --lib validate::
