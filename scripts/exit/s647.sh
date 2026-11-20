#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s647 exit test: a red base freezes landing (the freeze tests still pass).
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -q -p mason-daemon --test it freezes
