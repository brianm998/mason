#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s572 exit test: the Compose core tests pass.
set -uo pipefail
cd "$(dirname "$0")/../../clients/compose" && gradle :core:test --no-daemon -q
