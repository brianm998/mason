#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# The review_validate tests answer round 1's blocking finding in their round-2 approval (s1257).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --test it review_validate::
