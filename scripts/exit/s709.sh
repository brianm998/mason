#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s709 exit test: the github adapter test, including the fake host kit, passes.
set -uo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-adapters --test github
