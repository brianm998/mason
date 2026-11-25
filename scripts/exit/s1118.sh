#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1118 exit test: the coordinator runs masond from one stable, signed bundle on the internal
# disk, and a Local Network rule macOS cannot apply is named as such (decision 1107).
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1118.md
# Two takeovers in a scratch data dir with two fake runtimes: the same exec path both times,
# under the configured root, and `codesign --verify --strict` passes (macOS only; empty elsewhere).
cargo test -q -p mason-daemon --test stable_bundle
cargo test -q -p mason-daemon --lib stable_app::
cargo test -q -p mason-daemon --lib no_route_to_a_private_address_that_pings
echo "OK s1118"
