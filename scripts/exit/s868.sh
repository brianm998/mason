#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s868 exit test: the owner's self-update build takes its `compile` slot ahead of task work and
# past the heat hold, up to a hard ceiling; an automatic one rises after `update_wait_max_s`.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -p mason-daemon --lib -- an_owners_update_build an_automatic_update_build \
  an_update_goes_ahead update_wait_max_defaults
echo "s868 passed"
